use super::*;
use crate::test_support::PgWorkerTestDb;

#[tokio::test]
async fn postgres_offline_transition_persists_edges_and_marks_tunnels_unknown() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    insert_lifecycle_client(&db.pool, "edge-offline", "online", true).await;
    insert_lifecycle_client(&db.pool, "edge-peer", "online", false).await;

    let plan_id = Uuid::new_v4();
    let input = vpsman_common::TunnelPlanInput {
        name: "offline-test-tunnel".to_string(),
        interface_name: "gre-offline".to_string(),
        kind: vpsman_common::TunnelKind::Gre,
        runtime_control: vpsman_common::RuntimeTunnelControl {
            manager: vpsman_common::RuntimeTunnelManager::CustomAdapter,
            left_adapter_definition_id: Some("11111111-1111-4111-8111-111111111111".to_string()),
            right_adapter_definition_id: Some("22222222-2222-4222-8222-222222222222".to_string()),
            ..Default::default()
        },
        runtime_topology: Default::default(),
        left_client_id: "edge-offline".to_string(),
        right_client_id: "edge-peer".to_string(),
        left_remote_underlay: "198.51.100.10".to_string(),
        left_local_underlay: None,
        right_remote_underlay: "203.0.113.20".to_string(),
        right_local_underlay: None,
        address_pool_cidr: "10.88.0.0/30".to_string(),
        reserved_addresses: Vec::new(),
        ipv4_tunnel: Some(vpsman_common::TunnelAddressPair {
            left: "10.88.0.0".to_string(),
            right: "10.88.0.1".to_string(),
            prefix_len: 31,
        }),
        ipv6_address_pool_cidr: None,
        ipv6_tunnel: None,
        latency_primary_family: Default::default(),
        bandwidth_mbps: 100,
        left_mtu: None,
        right_mtu: None,
        ospf: None,
    };
    let plan = vpsman_common::plan_tunnel(&input).unwrap();
    sqlx::query(
        r#"
        INSERT INTO tunnel_plans (
            id, name, kind, enabled, left_client_id, right_client_id, input, plan
        ) VALUES ($1, $2, 'gre', TRUE, $3, $4, $5, $6)
        "#,
    )
    .bind(plan_id)
    .bind(&input.name)
    .bind(&input.left_client_id)
    .bind(&input.right_client_id)
    .bind(SqlJson(&input))
    .bind(SqlJson(&plan))
    .execute(&db.pool)
    .await
    .unwrap();
    let runtime_identity =
        vpsman_common::tunnel_runtime_evidence_identity_hash(plan_id, &plan, None);
    let tunnel_episode_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO operational_alert_episodes (
            id, public_id, producer_kind, natural_key, record_kind,
            trigger_generation, trigger_severity, trigger_category,
            severity, category, target_kind, target_id, client_id,
            title, detail, source_status, evidence, lifecycle_state,
            triggered_at, last_confirmed_at
        ) VALUES (
            $1, $2, 'tunnel_traffic', $3, 'condition',
            1, 'warning', 'network', 'warning', 'network', 'tunnel',
            'edge-offline:gre-offline', 'edge-offline',
            'Tunnel interface counters are degraded', 'counter read failed',
            'tunnel_traffic_degraded', '{}'::jsonb, 'triggered',
            clock_timestamp(), clock_timestamp()
        )
        "#,
    )
    .bind(tunnel_episode_id)
    .bind(format!("operational-alert:{tunnel_episode_id}"))
    .bind(format!("{plan_id}:{runtime_identity}:left"))
    .execute(&db.pool)
    .await
    .unwrap();
    let retained_episode_id = Uuid::new_v4();
    let retained_public_id = "network:tunnel:retained-worker";
    sqlx::query(
        r#"
        INSERT INTO operational_alert_episodes (
            id, public_id, producer_kind, natural_key, record_kind,
            trigger_generation, trigger_severity, trigger_category,
            severity, category, target_kind, target_id, client_id,
            title, detail, source_status, evidence, lifecycle_state,
            triggered_at, last_confirmed_at, backfilled
        ) VALUES (
            $1, $2, 'tunnel_adapter', $3, 'condition',
            1, 'critical', 'network', 'critical', 'network', 'tunnel',
            'edge-offline:gre-offline', 'edge-offline',
            'Tunnel adapter status failed', 'retained adapter failure',
            'tunnel_adapter_degraded',
            '{"retain_unknown_backfill":true,"status_boundary_at":"2000-01-01T00:00:00Z","runtime_boundary_at":"2000-01-01T00:00:00Z"}'::jsonb,
            'unknown', '2000-01-01T00:00:00Z', '2000-01-01T00:00:00Z', TRUE
        )
        "#,
    )
    .bind(retained_episode_id)
    .bind(retained_public_id)
    .bind(format!("{plan_id}:{runtime_identity}:left"))
    .execute(&db.pool)
    .await
    .unwrap();

    assert_eq!(detect_offline_agents(&db.pool, 60).await.unwrap(), 1);
    let agent = sqlx::query(
        r#"
        SELECT id, lifecycle_state, trigger_generation, source_status
        FROM operational_alert_episodes
        WHERE producer_kind = 'agent_status' AND client_id = 'edge-offline'
          AND resolved_at IS NULL
        "#,
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let agent_episode_id: Uuid = agent.try_get("id").unwrap();
    assert_eq!(
        agent.try_get::<String, _>("lifecycle_state").unwrap(),
        "triggered"
    );
    assert_eq!(agent.try_get::<i64, _>("trigger_generation").unwrap(), 1);
    assert_eq!(
        agent.try_get::<String, _>("source_status").unwrap(),
        "offline"
    );

    let tunnel = sqlx::query(
        r#"
        SELECT lifecycle_state, source_status, evidence
        FROM operational_alert_episodes
        WHERE id = $1
        "#,
    )
    .bind(tunnel_episode_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        tunnel.try_get::<String, _>("lifecycle_state").unwrap(),
        "unknown"
    );
    assert_eq!(
        tunnel.try_get::<String, _>("source_status").unwrap(),
        "tunnel_traffic_evidence_missing"
    );
    let tunnel_evidence: Value = tunnel.try_get("evidence").unwrap();
    assert!(tunnel_evidence["status_boundary_at"].is_string());
    assert_eq!(
        tunnel_evidence["topology_identity_validation"],
        "unavailable"
    );
    let retained = sqlx::query(
        r#"
        SELECT public_id, lifecycle_state, title, detail, source_status, evidence
        FROM operational_alert_episodes
        WHERE id = $1
        "#,
    )
    .bind(retained_episode_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        retained.try_get::<String, _>("public_id").unwrap(),
        retained_public_id
    );
    assert_eq!(
        retained.try_get::<String, _>("lifecycle_state").unwrap(),
        "unknown"
    );
    assert_eq!(
        retained.try_get::<String, _>("title").unwrap(),
        "Tunnel adapter status failed"
    );
    assert_eq!(
        retained.try_get::<String, _>("detail").unwrap(),
        "retained adapter failure"
    );
    assert_eq!(
        retained.try_get::<String, _>("source_status").unwrap(),
        "tunnel_adapter_degraded"
    );
    let retained_evidence: Value = retained.try_get("evidence").unwrap();
    assert_eq!(retained_evidence["retain_unknown_backfill"], true);
    assert_ne!(
        retained_evidence["status_boundary_at"],
        "2000-01-01T00:00:00Z"
    );
    assert_ne!(
        retained_evidence["runtime_boundary_at"],
        "2000-01-01T00:00:00Z"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM webhook_events WHERE event_id LIKE $1",)
            .bind(format!("fleet-alert:{retained_episode_id}:%"))
            .fetch_one(&db.pool)
            .await
            .unwrap(),
        0,
        "marking retained tunnel evidence Unknown must not synthesize an edge"
    );

    let trigger = lifecycle_event(&db.pool, agent_episode_id, "triggered").await;
    assert_eq!(trigger["alert"]["producer_kind"], "agent_status");
    assert_eq!(trigger["alert"]["lifecycle_state"], "triggered");
    assert_eq!(trigger["alert"]["trigger_generation"], 1);
    assert_eq!(trigger["alert"]["source_status"], "offline");
    let trigger_contract = sqlx::query(
        "SELECT event_predicates, subject_client_ids FROM webhook_events WHERE event_id = $1",
    )
    .bind(format!("fleet-alert:{agent_episode_id}:triggered"))
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        trigger_contract
            .try_get::<Vec<String>, _>("event_predicates")
            .unwrap(),
        vec![
            "alert.category:agent_status",
            "alert.open",
            "alert.severity:critical",
            "alert.triggered",
        ]
    );
    assert_eq!(
        trigger_contract
            .try_get::<Vec<String>, _>("subject_client_ids")
            .unwrap(),
        vec!["edge-offline"]
    );

    let mut online = db.pool.begin().await.unwrap();
    sqlx::query(
        "UPDATE clients SET status = 'online', last_seen_at = clock_timestamp() WHERE id = $1",
    )
    .bind("edge-offline")
    .execute(&mut *online)
    .await
    .unwrap();
    reconcile_agent_status_transition_in_tx(&mut online, "edge-offline", "online")
        .await
        .unwrap();
    online.commit().await.unwrap();

    let resolved = sqlx::query(
        r#"
        SELECT lifecycle_state, resolution_reason, resolved_at IS NOT NULL AS resolved
        FROM operational_alert_episodes
        WHERE id = $1
        "#,
    )
    .bind(agent_episode_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        resolved.try_get::<String, _>("lifecycle_state").unwrap(),
        "resolved"
    );
    assert_eq!(
        resolved
            .try_get::<Option<String>, _>("resolution_reason")
            .unwrap()
            .as_deref(),
        Some("condition_recovered")
    );
    assert!(resolved.try_get::<bool, _>("resolved").unwrap());
    let resolution = lifecycle_event(&db.pool, agent_episode_id, "resolved").await;
    assert_eq!(resolution["alert"]["lifecycle_state"], "resolved");
    assert_eq!(
        resolution["alert"]["resolution_reason"],
        "condition_recovered"
    );
    let resolution_contract = sqlx::query(
        "SELECT event_predicates, subject_client_ids FROM webhook_events WHERE event_id = $1",
    )
    .bind(format!("fleet-alert:{agent_episode_id}:resolved"))
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        resolution_contract
            .try_get::<Vec<String>, _>("event_predicates")
            .unwrap(),
        vec![
            "alert.category:agent_status",
            "alert.resolved",
            "alert.severity:critical",
        ]
    );
    assert_eq!(
        resolution_contract
            .try_get::<Vec<String>, _>("subject_client_ids")
            .unwrap(),
        vec!["edge-offline"]
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT lifecycle_state FROM operational_alert_episodes WHERE id = $1",
        )
        .bind(tunnel_episode_id)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        "unknown",
        "reconnect does not manufacture fresh tunnel evidence"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM webhook_events WHERE event_id LIKE $1",)
            .bind(format!("fleet-alert:{agent_episode_id}:%"))
            .fetch_one(&db.pool)
            .await
            .unwrap(),
        2,
        "offline and immediate recovery must retain both durable lifecycle edges"
    );

    db.cleanup().await;
}

async fn insert_lifecycle_client(pool: &PgPool, client_id: &str, status: &str, stale: bool) {
    let public_key = hex::decode(vpsman_common::payload_hash(client_id.as_bytes())).unwrap();
    sqlx::query(
        r#"
        INSERT INTO clients (id, display_name, public_key, status, last_seen_at, capabilities)
        VALUES ($1, $1, $2, $3,
                CASE WHEN $4 THEN clock_timestamp() - interval '1 hour' ELSE clock_timestamp() END,
                '{}'::jsonb)
        "#,
    )
    .bind(client_id)
    .bind(public_key)
    .bind(status)
    .bind(stale)
    .execute(pool)
    .await
    .unwrap();
}

async fn lifecycle_event(pool: &PgPool, episode_id: Uuid, state: &str) -> Value {
    sqlx::query_scalar::<_, SqlJson<Value>>(
        "SELECT payload FROM webhook_events WHERE kind = $1 AND event_id = $2",
    )
    .bind(format!("alert.{state}"))
    .bind(format!("fleet-alert:{episode_id}:{state}"))
    .fetch_one(pool)
    .await
    .unwrap()
    .0
}
