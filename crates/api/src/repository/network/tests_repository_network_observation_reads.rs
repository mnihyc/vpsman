use super::*;
use sqlx::{postgres::PgPoolOptions, PgConnection, PgPool};
use std::{path::Path, str::FromStr};

const END: i64 = 2_000_000_000;

async fn database() -> Option<(PgPool, PgPool, String)> {
    let url = std::env::var("VPSMAN_TEST_POSTGRES_URL").ok()?;
    if url.trim().is_empty() {
        return None;
    }
    let options = sqlx::postgres::PgConnectOptions::from_str(&url).unwrap();
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(options.clone().database("postgres"))
        .await
        .unwrap();
    let name = format!("vpsman_exact_read_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE DATABASE {name}"))
        .execute(&admin)
        .await
        .unwrap();
    let options = options.database(&name);
    crate::repository::migrate_postgres_database(
        &options,
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../migrations"),
    )
    .await
    .unwrap();
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(options.options([("search_path", "public"), ("jit", "off")]))
        .await
        .unwrap();
    Some((admin, pool, name))
}

async fn cleanup(admin: PgPool, pool: PgPool, name: String) {
    pool.close().await;
    sqlx::query(&format!("DROP DATABASE {name}"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}

async fn seed(pool: &PgPool, samples: i32) {
    sqlx::raw_sql(
        r#"
        INSERT INTO clients(id,display_name,public_key,status,hidden_at,
            suspended_at,suspended_from_status)
        SELECT id,id,decode('', 'hex'),
               CASE id WHEN 'read-suspended' THEN 'suspended'
                       WHEN 'read-deleted' THEN 'deleted' ELSE 'online' END,
               CASE WHEN id = 'read-deleted' THEN now() END,
               CASE WHEN id = 'read-suspended' THEN now() END,
               CASE WHEN id = 'read-suspended' THEN 'offline' END
        FROM unnest(ARRAY['read-left','read-right','read-suspended','read-deleted']) id;
        INSERT INTO tunnel_plans(id,name,kind,left_client_id,right_client_id,input,plan,deleted_at)
        SELECT md5('read-plan-' || n)::uuid, 'read-plan-' || n, 'wireguard',
               CASE WHEN n = 3 THEN 'read-suspended' ELSE 'read-left' END,
               'read-right','{}','{}',CASE WHEN n = 4 THEN now() END
        FROM generate_series(1,4) n;
        INSERT INTO network_observation_series(plan_id,topology_identity_hash,plan_name,
            interface_name,client_id,peer_client_id,endpoint_side,address_family,target)
        SELECT md5('read-plan-' || n)::uuid, identity, 'renamed-series-' || n,
               'tun0', 'read-' || side,
               CASE side WHEN 'left' THEN 'read-right' ELSE 'read-left' END,
               side, family, CASE family WHEN 'ipv4' THEN '10.0.0.2' ELSE 'fd00::2' END
        FROM generate_series(1,2) n
        CROSS JOIN (VALUES ('identity'),('old-identity')) identities(identity)
        CROSS JOIN (VALUES ('left'),('right')) endpoints(side)
        CROSS JOIN (VALUES ('ipv4'),('ipv6')) families(family);
        "#,
    )
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO telemetry_samples(id,client_id,observed_at,cpu_cores,
            cpu_load_1,cpu_load_5,cpu_load_15,memory_total_bytes,memory_available_bytes,
            disk_total_bytes,disk_available_bytes,tcp_sockets,udp_sockets,payload,
            accepted_seq,accepted_at,source_gateway_id,source_gateway_session_id,
            source_process_incarnation_id,source_telemetry_seq,reported_observed_unix)
        SELECT md5(client.id || ':sample:' || n)::uuid,client.id,
            to_timestamp(2000000000-$1+n),2,0,0,0,1024,512,4096,2048,0,0,
            jsonb_build_object('tunnel_reachability', (
                SELECT jsonb_agg(jsonb_build_object(
                    'id',md5(series.id || ':observation:' || n)::uuid,
                    'stale_after_secs',180,'healthy',n%3<>0,
                    'transmitted',3,'received',CASE WHEN n%3=0 THEN 0 ELSE 3 END,
                    'latency_min_ms',4.0,'latency_avg_ms',5.0+n%10,'latency_max_ms',15.0,
                    'latency_mdev_ms',1.0,'packet_loss_ratio',CASE WHEN n%3=0 THEN 1.0 ELSE 0.0 END,
                    'reason',CASE WHEN n%3=0 THEN 'fixture loss' END
                ) ORDER BY series.id)
                FROM network_observation_series series WHERE series.client_id=client.id
            ), 'other_telemetry', repeat('host-interface-evidence ',100)),
            n,now(),'read-test',md5(client.id || ':gateway')::uuid,
            md5(client.id || ':process')::uuid,n,2000000000-$1+n
        FROM clients client CROSS JOIN generate_series(1,$1::integer) n
        WHERE client.id IN ('read-left','read-right')
        "#,
    )
    .bind(samples)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO network_observations(id,plan_name,source,automatic_series_id,
            automatic_sample_id,automatic_payload_ordinal,observed_at,received_at)
        SELECT md5(series.id || ':observation:' || n)::uuid,'historical-raw-label',
            'automatic',series.id,md5(series.client_id || ':sample:' || n)::uuid,
            series.ordinal::smallint,to_timestamp(2000000000-$1+n),
            to_timestamp(2000000000-$1+n)
        FROM (
            SELECT id,client_id,row_number() OVER (PARTITION BY client_id ORDER BY id) ordinal
            FROM network_observation_series
        ) series CROSS JOIN generate_series(1,$1::integer) n
        "#,
    )
    .bind(samples)
    .execute(pool)
    .await
    .unwrap();
}

fn filter() -> NetworkObservationFilter {
    NetworkObservationFilter {
        start_unix: END - 86_400,
        end_unix: END,
        plan_ids: Vec::new(),
        client_id: None,
        source: None,
        kind: None,
        health: None,
        search: None,
        limit: 2,
        visible_only: true,
    }
}

// Deliberately unbounded reference: rank the canonical exact view, as the
// previous readers did. Do not use the production candidate builder here.
fn oracle(graph: bool) -> String {
    let partition = if graph {
        ""
    } else {
        "observation.topology_identity_hash,"
    };
    let order = if graph { "" } else { "evidence_rank," };
    let visibility = if graph {
        r#"NOT EXISTS (
            SELECT 1 FROM visible_clients suspended_client
            WHERE suspended_client.status = 'suspended'
              AND (suspended_client.id = observation.client_id
                   OR suspended_client.id = observation.peer_client_id)
        ) AND NOT EXISTS (
            SELECT 1 FROM tunnel_plans plan
            JOIN visible_clients suspended_endpoint ON suspended_endpoint.status = 'suspended'
              AND suspended_endpoint.id IN (plan.left_client_id, plan.right_client_id)
            WHERE plan.id = observation.plan_id
        )"#
    } else {
        r#"EXISTS (SELECT 1 FROM visible_clients WHERE id = observation.client_id AND status <> 'suspended')
        AND (observation.peer_client_id IS NULL OR EXISTS (
            SELECT 1 FROM visible_clients WHERE id = observation.peer_client_id AND status <> 'suspended'
        )) AND EXISTS (
            SELECT 1 FROM tunnel_plans plan
            WHERE plan.id = observation.plan_id AND plan.deleted_at IS NULL
              AND NOT EXISTS (SELECT 1 FROM visible_clients endpoint
                  WHERE endpoint.id = plan.left_client_id AND endpoint.status = 'suspended')
              AND NOT EXISTS (SELECT 1 FROM visible_clients endpoint
                  WHERE endpoint.id = plan.right_client_id AND endpoint.status = 'suspended')
        )"#
    };
    let kinds = if graph {
        "AND observation.kind IN ('tunnel_reachability','network_speed_test','network_status')"
    } else {
        ""
    };
    format!(
        r#"
        WITH ranked AS (
            SELECT observation.*, row_number() OVER (
                PARTITION BY observation.plan_id, {partition} observation.kind,
                    COALESCE(observation.endpoint_side, observation.client_id)
                ORDER BY observation.observed_at DESC, observation.id DESC
            ) evidence_rank
            FROM network_observation_exact_evidence observation
            WHERE observation.observed_at >= to_timestamp($1)
              AND observation.observed_at <= to_timestamp($2)
              AND (cardinality($3::uuid[]) = 0 OR observation.plan_id = ANY($3::uuid[]))
              AND ($4::text IS NULL OR observation.client_id = $4 OR observation.peer_client_id = $4)
              AND ($5::text IS NULL OR observation.source = $5)
              AND ($6::text IS NULL OR observation.kind = $6)
              {kinds}
              AND ($7::text IS NULL OR ($7 = 'healthy' AND observation.healthy IS TRUE)
                  OR ($7 = 'unhealthy' AND observation.healthy IS FALSE)
                  OR ($7 = 'unknown' AND observation.healthy IS NULL))
              AND ($8::text IS NULL OR concat_ws(' ',observation.client_id,observation.peer_client_id,
                  observation.plan_name,observation.interface_name,observation.target,
                  observation.reason,observation.kind,observation.source) ILIKE '%' || $8 || '%')
              AND (NOT $9 OR ({visibility}))
        )
        SELECT {OBSERVATION_COLUMNS} FROM ranked WHERE evidence_rank <= $10
        ORDER BY {order} observed_at DESC, id DESC LIMIT $11
        "#
    )
}

async fn query_rows(
    connection: &mut PgConnection,
    sql: &str,
    filter: &NetworkObservationFilter,
    cap: Option<i64>,
) -> serde_json::Value {
    let rows = sqlx::query(sql)
        .bind(filter.start_unix)
        .bind(filter.end_unix)
        .bind(&filter.plan_ids)
        .bind(filter.client_id.as_deref())
        .bind(filter.source.as_deref())
        .bind(filter.kind.as_deref())
        .bind(filter.health.as_deref())
        .bind(filter.search.as_deref())
        .bind(filter.visible_only)
        .bind(filter.limit)
        .bind(cap)
        .fetch_all(connection)
        .await
        .unwrap()
        .into_iter()
        .map(network_observation_from_row)
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    serde_json::to_value(rows).unwrap()
}

#[tokio::test]
async fn postgres_bounded_exact_reads_preserve_view_filters_ranking_and_fallback() {
    let Some((admin, pool, name)) = database().await else {
        return;
    };
    seed(&pool, 6).await;
    sqlx::raw_sql(
        r#"
        INSERT INTO network_observations(id,client_id,peer_client_id,kind,source,
            plan_id,topology_identity_hash,plan_name,interface_name,endpoint_side,
            healthy,reason,metadata,observed_at,received_at)
        SELECT gen_random_uuid(),'read-left',peer,kinds.kind,'manual',plan.id,
            'identity','manual-label','tun0',side,healthy,'Manual Diagnostic',
            '{"nested":{"preserved":true}}',
            to_timestamp(1999999998 + CASE WHEN healthy IS NULL THEN 0.25 ELSE 0 END),
            to_timestamp(1999999999)
        FROM tunnel_plans plan
        CROSS JOIN (VALUES ('network_status'),('network_speed_test'),('tunnel_reachability'),('other')) kinds(kind)
        CROSS JOIN (VALUES ('left'::text),(NULL::text)) sides(side)
        CROSS JOIN (VALUES (true),(false),(NULL::boolean)) health(healthy)
        CROSS JOIN (VALUES ('read-right'),('read-deleted'),('read-suspended')) peers(peer);
        -- Newest missing/incorrect payload slots must not consume a physical cap.
        UPDATE network_observations SET automatic_payload_ordinal=automatic_payload_ordinal+100
        WHERE automatic_series_id IS NOT NULL AND observed_at=to_timestamp(2000000000);
        UPDATE telemetry_samples
        SET payload=jsonb_set(payload,'{tunnel_reachability,0,id}',to_jsonb(md5('wrong-observation')::uuid))
        WHERE observed_at=to_timestamp(1999999999);
        UPDATE telemetry_samples
        SET payload=jsonb_set(payload,'{tunnel_reachability,0,healthy}','null')
        WHERE observed_at=to_timestamp(1999999997);
        -- Inactive series retain raw history; only their fallback is excluded.
        UPDATE network_observation_series SET active=false WHERE address_family='ipv6';
        INSERT INTO network_observation_latest(series_id,observation_id,stale_after_secs,
            healthy,transmitted,received,packet_loss_ratio,reason,metadata,observed_at,received_at)
        SELECT id,
            CASE WHEN endpoint_side='left' THEN md5(id || ':observation:6')::uuid
                 ELSE md5(id || ':fallback')::uuid END,
            180,true,3,3,0,'Latest Diagnostic','{"latest":true}',
            to_timestamp(2000000000),to_timestamp(2000000000)
        FROM network_observation_series;
        -- Even an out-of-window invalid locator shadows its active latest UUID.
        UPDATE network_observations SET observed_at=to_timestamp(1900000000)
        WHERE id IN (SELECT observation_id FROM network_observation_latest);
        "#,
    )
    .execute(&pool)
    .await
    .unwrap();
    let plans: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM tunnel_plans ORDER BY name")
        .fetch_all(&pool)
        .await
        .unwrap();
    let mut cases = vec![filter()];
    for ids in [
        vec![plans[0]],
        vec![plans[1]],
        vec![plans[2]],
        vec![plans[3]],
        vec![Uuid::new_v4()],
    ] {
        cases.push(NetworkObservationFilter {
            plan_ids: ids,
            ..filter()
        });
    }
    for client in [
        "read-left",
        "read-right",
        "read-deleted",
        "read-suspended",
        "absent",
    ] {
        cases.push(NetworkObservationFilter {
            client_id: Some(client.into()),
            ..filter()
        });
    }
    for source in ["automatic", "manual", "absent"] {
        cases.push(NetworkObservationFilter {
            source: Some(source.into()),
            ..filter()
        });
    }
    for kind in [
        "tunnel_reachability",
        "network_status",
        "network_speed_test",
        "other",
    ] {
        cases.push(NetworkObservationFilter {
            kind: Some(kind.into()),
            ..filter()
        });
    }
    for health in ["healthy", "unhealthy", "unknown"] {
        cases.push(NetworkObservationFilter {
            health: Some(health.into()),
            ..filter()
        });
    }
    for search in [
        "historical-raw",
        "renamed-series",
        "manual-label",
        "DIAGNOSTIC",
        "fixture loss",
        "%",
        "_",
        "absent",
    ] {
        cases.push(NetworkObservationFilter {
            search: Some(search.into()),
            ..filter()
        });
    }
    cases.push(NetworkObservationFilter {
        start_unix: END - 3,
        end_unix: END - 3,
        ..filter()
    });
    cases.push(NetworkObservationFilter {
        start_unix: END + 1,
        end_unix: END + 10,
        ..filter()
    });
    let mut connection = pool.acquire().await.unwrap();
    for mode in ["force_custom_plan", "force_generic_plan"] {
        sqlx::query(&format!("SET plan_cache_mode={mode}"))
            .execute(&mut *connection)
            .await
            .unwrap();
        for graph in [false, true] {
            let reference = oracle(graph);
            let bounded = exact_reads::query(graph);
            for original in &cases {
                for visible in [false, true] {
                    for limit in [1, 3] {
                        let case = NetworkObservationFilter {
                            visible_only: visible,
                            limit,
                            ..original.clone()
                        };
                        for cap in [None, Some(3)] {
                            let expected =
                                query_rows(&mut connection, &reference, &case, cap).await;
                            let actual = query_rows(&mut connection, &bounded, &case, cap).await;
                            assert_eq!(
                                actual, expected,
                                "mode={mode} graph={graph} cap={cap:?} filter={case:?}"
                            );
                        }
                    }
                }
            }
        }
    }
    // Exercise production bindings, not only the SQL builder. Graph must keep
    // its existing post-cap current-identity filter without refilling old rows.
    sqlx::query("UPDATE network_observations SET topology_identity_hash='old-identity', observed_at=to_timestamp($1) WHERE source='manual'")
        .bind(END).execute(&mut *connection).await.unwrap();
    let case = NetworkObservationFilter {
        plan_ids: vec![plans[0]],
        limit: 1,
        ..filter()
    };
    let expected_fair = query_rows(&mut connection, &oracle(false), &case, Some(250_000)).await;
    let graph_expected = query_rows(&mut connection, &oracle(true), &case, None).await;
    drop(connection);
    let repo = Repository::Postgres(pool.clone());
    let actual_fair = repo
        .list_network_observations_filtered(&case)
        .await
        .unwrap();
    assert_eq!(serde_json::to_value(actual_fair).unwrap(), expected_fair);
    let actual_graph = repo
        .list_network_observations_for_topology(
            &[(
                plans[0],
                "identity".into(),
                "read-left".into(),
                "read-right".into(),
            )],
            case.start_unix,
            case.end_unix,
            1,
        )
        .await
        .unwrap();
    let expected_graph: Vec<_> = graph_expected
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["topology_identity_hash"] == "identity")
        .cloned()
        .collect();
    assert_eq!(
        serde_json::to_value(actual_graph).unwrap(),
        serde_json::json!(expected_graph)
    );
    cleanup(admin, pool, name).await;
}

fn plan_nodes<'a>(node: &'a serde_json::Value, nodes: &mut Vec<&'a serde_json::Value>) {
    nodes.push(node);
    if let Some(children) = node["Plans"].as_array() {
        for child in children {
            plan_nodes(child, nodes);
        }
    }
}

#[tokio::test]
async fn postgres_bounded_exact_reads_stop_payload_reads_at_each_physical_cap() {
    let Some((admin, pool, name)) = database().await else {
        return;
    };
    // 16 physical owners, 2,000 observations each; the request needs only seven
    // per logical group. Assert executor work, not a hardware-dependent timer.
    seed(&pool, 2_000).await;
    sqlx::raw_sql("ANALYZE network_observations; ANALYZE telemetry_samples; ANALYZE network_observation_series; ANALYZE network_observation_latest; ANALYZE clients; ANALYZE tunnel_plans;")
        .execute(&pool).await.unwrap();
    let mut connection = pool.acquire().await.unwrap();
    let case = NetworkObservationFilter {
        limit: 7,
        ..filter()
    };
    for mode in ["force_custom_plan", "force_generic_plan"] {
        sqlx::query(&format!("SET plan_cache_mode={mode}"))
            .execute(&mut *connection)
            .await
            .unwrap();
        for graph in [true, false] {
            // EXPLAIN EXECUTE inspects the actual prepared query plan; a
            // parameterized EXPLAIN SELECT can instead plan its utility body.
            let prepare = format!(
                "PREPARE exact_read_bound_test AS {}",
                exact_reads::query(graph)
            );
            sqlx::raw_sql(&prepare)
                .execute(&mut *connection)
                .await
                .unwrap();
            let explain = format!(
                "EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) EXECUTE exact_read_bound_test(\
                 {},{},'{{}}'::uuid[],NULL::text,NULL::text,NULL::text,NULL::text,\
                 NULL::text,true,{},NULL::bigint)",
                case.start_unix, case.end_unix, case.limit
            );
            let plan: serde_json::Value = sqlx::query_scalar(&explain)
                .fetch_one(&mut *connection)
                .await
                .unwrap();
            sqlx::raw_sql("DEALLOCATE exact_read_bound_test")
                .execute(&mut *connection)
                .await
                .unwrap();
            let mut nodes = Vec::new();
            plan_nodes(&plan[0]["Plan"], &mut nodes);
            let payload_rows: u64 = nodes
                .iter()
                .filter(|node| node["Relation Name"] == "telemetry_samples")
                .map(|node| {
                    node["Actual Rows"].as_u64().unwrap() * node["Actual Loops"].as_u64().unwrap()
                })
                .sum();
            assert_eq!(payload_rows, 16 * 7, "{mode} graph={graph}: {plan}");
            assert!(
                nodes.iter().any(|node| node["Index Name"]
                    == "network_observations_automatic_series_observed_idx"),
                "{mode}: {plan}"
            );
            let expected = query_rows(&mut connection, &oracle(graph), &case, None).await;
            let actual = query_rows(&mut connection, &exact_reads::query(graph), &case, None).await;
            assert_eq!(actual, expected, "{mode} graph={graph}");
        }
    }
    drop(connection);
    cleanup(admin, pool, name).await;
}
