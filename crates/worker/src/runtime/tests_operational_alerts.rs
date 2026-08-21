use super::*;
use crate::test_support::PgWorkerTestDb;
use serde_json::{json, Value};
use sqlx::{types::Json as SqlJson, PgPool, Row};
use uuid::Uuid;

#[tokio::test]
async fn postgres_offline_transition_records_neutral_policy_evidence() {
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

    assert_eq!(detect_offline_agents(&db.pool, 60).await.unwrap(), 1);
    let agent_status = sqlx::query(
        r#"
        SELECT evidence_seq, source_event_id, observed_at, fact_kind, natural_key, confirmation_bucket_key,
               subject_client_id, target_kind, target_id, source_status,
               completeness, subject_snapshot, payload,
               state_started_at = observed_at AS state_boundary_matches
        FROM alert_policy_evidence
        WHERE source_kind = 'agent.status'
          AND natural_key = 'edge-offline:connectivity'
        "#,
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        agent_status.try_get::<String, _>("fact_kind").unwrap(),
        "state"
    );
    assert_eq!(
        agent_status.try_get::<String, _>("natural_key").unwrap(),
        "edge-offline:connectivity"
    );
    assert_eq!(
        agent_status
            .try_get::<String, _>("confirmation_bucket_key")
            .unwrap(),
        "edge-offline:connectivity"
    );
    assert_eq!(
        agent_status
            .try_get::<Option<String>, _>("subject_client_id")
            .unwrap()
            .as_deref(),
        Some("edge-offline")
    );
    assert_eq!(
        agent_status.try_get::<String, _>("target_kind").unwrap(),
        "agent"
    );
    assert_eq!(
        agent_status.try_get::<String, _>("target_id").unwrap(),
        "edge-offline"
    );
    assert_eq!(
        agent_status.try_get::<String, _>("source_status").unwrap(),
        "offline"
    );
    assert_eq!(
        agent_status.try_get::<String, _>("completeness").unwrap(),
        "complete"
    );
    assert!(agent_status
        .try_get::<bool, _>("state_boundary_matches")
        .unwrap());
    let agent_subject: Value = agent_status.try_get("subject_snapshot").unwrap();
    assert_eq!(agent_subject["client_id"], "edge-offline");
    assert_eq!(agent_subject["display_name"], "edge-offline");
    assert_eq!(agent_subject["status"], "offline");
    assert_eq!(agent_subject["tags"], json!([]));
    assert_eq!(agent_subject["scope_complete"], true);
    let agent_payload: Value = agent_status.try_get("payload").unwrap();
    assert_eq!(agent_payload["status"], "offline");
    assert_eq!(agent_payload["source_status"], "offline");
    assert_eq!(agent_payload["client_id"], "edge-offline");
    assert_eq!(
        agent_payload["reason"],
        "edge-offline currently reports offline"
    );
    let status_boundary = sqlx::query_scalar::<_, chrono::DateTime<chrono::Utc>>(
        "SELECT operational_alert_status_at FROM clients WHERE id='edge-offline'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        agent_status
            .try_get::<chrono::DateTime<chrono::Utc>, _>("observed_at")
            .unwrap(),
        status_boundary
    );
    assert_eq!(
        agent_status
            .try_get::<String, _>("source_event_id")
            .unwrap(),
        vpsman_common::alert_policy_state_source_event_id(
            "agent.status",
            "edge-offline:connectivity",
            status_boundary.timestamp_nanos_opt().unwrap(),
            &agent_payload,
        )
    );
    let offline_agent_status_seq = agent_status.try_get::<i64, _>("evidence_seq").unwrap();

    let agent_access = sqlx::query(
        r#"
        SELECT fact_kind, subject_client_id, target_kind, target_id,
               source_status, completeness, subject_snapshot, payload,
               state_started_at = observed_at AS state_boundary_matches
        FROM alert_policy_evidence
        WHERE source_kind = 'agent.access'
          AND natural_key = 'edge-offline:access'
        "#,
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        agent_access.try_get::<String, _>("fact_kind").unwrap(),
        "state"
    );
    assert_eq!(
        agent_access
            .try_get::<Option<String>, _>("subject_client_id")
            .unwrap()
            .as_deref(),
        Some("edge-offline")
    );
    assert_eq!(
        agent_access.try_get::<String, _>("target_kind").unwrap(),
        "agent"
    );
    assert_eq!(
        agent_access.try_get::<String, _>("target_id").unwrap(),
        "edge-offline"
    );
    assert_eq!(
        agent_access.try_get::<String, _>("source_status").unwrap(),
        "offline"
    );
    assert_eq!(
        agent_access.try_get::<String, _>("completeness").unwrap(),
        "complete"
    );
    assert!(agent_access
        .try_get::<bool, _>("state_boundary_matches")
        .unwrap());
    assert_eq!(
        agent_access
            .try_get::<Value, _>("subject_snapshot")
            .unwrap(),
        agent_subject
    );
    let access_payload: Value = agent_access.try_get("payload").unwrap();
    assert_eq!(access_payload["status"], "offline");
    assert_eq!(access_payload["source_status"], "offline");
    assert_eq!(access_payload["client_id"], "edge-offline");
    assert_eq!(
        access_payload["reason"],
        "edge-offline cannot reconnect until an operator assigns a new key"
    );

    let tunnel_natural_key = format!("{plan_id}:{runtime_identity}:left");
    for (source_kind, expected_source_status) in [
        ("tunnel.adapter", "tunnel_adapter_evidence_missing"),
        ("tunnel.traffic", "tunnel_traffic_evidence_missing"),
    ] {
        let evidence = sqlx::query(
            r#"
            SELECT fact_kind, natural_key, confirmation_bucket_key,
                   subject_client_id, target_kind, target_id, source_status,
                   completeness, subject_snapshot, payload,
                   state_started_at = observed_at AS state_boundary_matches
            FROM alert_policy_evidence
            WHERE source_kind = $1 AND natural_key = $2
            "#,
        )
        .bind(source_kind)
        .bind(&tunnel_natural_key)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(evidence.try_get::<String, _>("fact_kind").unwrap(), "state");
        assert_eq!(
            evidence.try_get::<String, _>("natural_key").unwrap(),
            tunnel_natural_key
        );
        assert_eq!(
            evidence
                .try_get::<String, _>("confirmation_bucket_key")
                .unwrap(),
            tunnel_natural_key
        );
        assert_eq!(
            evidence
                .try_get::<Option<String>, _>("subject_client_id")
                .unwrap()
                .as_deref(),
            Some("edge-offline")
        );
        assert_eq!(
            evidence.try_get::<String, _>("target_kind").unwrap(),
            "tunnel"
        );
        assert_eq!(
            evidence.try_get::<String, _>("target_id").unwrap(),
            "edge-offline:gre-offline"
        );
        assert_eq!(
            evidence.try_get::<String, _>("source_status").unwrap(),
            expected_source_status
        );
        assert_eq!(
            evidence.try_get::<String, _>("completeness").unwrap(),
            "unknown"
        );
        assert!(evidence
            .try_get::<bool, _>("state_boundary_matches")
            .unwrap());
        assert_eq!(
            evidence.try_get::<Value, _>("subject_snapshot").unwrap(),
            agent_subject
        );

        let payload: Value = evidence.try_get("payload").unwrap();
        assert_eq!(payload["status"], expected_source_status);
        assert_eq!(payload["source_status"], expected_source_status);
        assert_eq!(payload["client_id"], "edge-offline");
        assert_eq!(payload["interface"], "gre-offline");
        assert_eq!(payload["plan"]["interface"], "gre-offline");
        assert!(payload["status_boundary_at"].is_string());
        assert!(payload["runtime_boundary_at"].is_string());
        assert_eq!(payload["topology_identity_validation"], "unavailable");
        match source_kind {
            "tunnel.adapter" => {
                assert!(payload["adapter"].is_null());
                assert!(payload["adapter_health"].is_null());
            }
            "tunnel.traffic" => {
                assert_eq!(payload["traffic"], json!({"status": null}));
                assert!(payload["traffic_status"].is_null());
            }
            _ => unreachable!(),
        }
    }

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

    let online_agent_status = sqlx::query(
        r#"
        SELECT evidence_seq, fact_kind, source_status, completeness, payload,
               state_started_at = observed_at AS state_boundary_matches
        FROM alert_policy_evidence
        WHERE source_kind = 'agent.status'
          AND natural_key = 'edge-offline:connectivity'
        ORDER BY evidence_seq DESC
        LIMIT 1
        "#,
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        online_agent_status
            .try_get::<String, _>("fact_kind")
            .unwrap(),
        "state"
    );
    assert_eq!(
        online_agent_status
            .try_get::<String, _>("source_status")
            .unwrap(),
        "online"
    );
    assert_eq!(
        online_agent_status
            .try_get::<String, _>("completeness")
            .unwrap(),
        "complete"
    );
    assert!(online_agent_status
        .try_get::<bool, _>("state_boundary_matches")
        .unwrap());
    assert!(
        online_agent_status
            .try_get::<i64, _>("evidence_seq")
            .unwrap()
            > offline_agent_status_seq,
        "the recovery fact must follow the offline fact in durable evidence order"
    );
    let online_payload: Value = online_agent_status.try_get("payload").unwrap();
    assert_eq!(online_payload["status"], "online");
    assert_eq!(online_payload["source_status"], "online");
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            r#"
            SELECT count(*)
            FROM alert_policy_evidence
            WHERE source_kind = 'agent.status'
              AND natural_key = 'edge-offline:connectivity'
            "#,
        )
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        2,
        "offline and online transitions must remain distinct durable facts"
    );

    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            r#"
            SELECT count(*)
            FROM webhook_events
            WHERE kind IN ('alert.triggered', 'alert.resolved')
               OR event_predicates && ARRAY['alert.triggered', 'alert.resolved']::text[]
            "#,
        )
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        0,
        "worker source transitions must not publish retired alert lifecycle aliases"
    );

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_offline_sweep_skips_locked_oldest_and_records_each_transition_once() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    let oldest_client_id = "offline-sweep-locked-oldest";
    let peer_client_id = "offline-sweep-unlocked-peer";
    insert_lifecycle_client(&db.pool, oldest_client_id, "online", true).await;
    insert_lifecycle_client(&db.pool, peer_client_id, "online", true).await;
    sqlx::query(
        r#"
        UPDATE clients
        SET last_seen_at = CASE id
            WHEN $1 THEN clock_timestamp() - interval '2 hours'
            ELSE clock_timestamp() - interval '1 hour'
        END
        WHERE id = ANY($2::text[])
        "#,
    )
    .bind(oldest_client_id)
    .bind(vec![
        oldest_client_id.to_string(),
        peer_client_id.to_string(),
    ])
    .execute(&db.pool)
    .await
    .unwrap();

    let mut oldest_holder = db.pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM clients WHERE id = $1 FOR UPDATE")
        .bind(oldest_client_id)
        .execute(&mut *oldest_holder)
        .await
        .unwrap();
    let first_sweep = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        detect_offline_agents(&db.pool, 60),
    )
    .await
    .expect("a locked oldest client blocked the offline sweep")
    .unwrap();
    assert_eq!(first_sweep, 1);
    let first_statuses: Vec<(String, String)> = sqlx::query_as(
        r#"
        SELECT id, status
        FROM clients
        WHERE id = ANY($1::text[])
        ORDER BY id
        "#,
    )
    .bind(vec![
        oldest_client_id.to_string(),
        peer_client_id.to_string(),
    ])
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        first_statuses,
        vec![
            (oldest_client_id.to_string(), "online".to_string()),
            (peer_client_id.to_string(), "offline".to_string()),
        ]
    );

    oldest_holder.rollback().await.unwrap();
    assert_eq!(
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            detect_offline_agents(&db.pool, 60),
        )
        .await
        .expect("released oldest client was not processed promptly")
        .unwrap(),
        1
    );
    assert_eq!(detect_offline_agents(&db.pool, 60).await.unwrap(), 0);
    for client_id in [oldest_client_id, peer_client_id] {
        assert_eq!(
            offline_side_effect_counts(&db.pool, client_id).await,
            (1, 1, 1, 1, 1),
            "offline transition side effects were not exactly once for {client_id}"
        );
    }

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_offline_sweep_does_not_pin_client_behind_reconcile_advisory() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    let client_id = "offline-sweep-advisory-held";
    insert_lifecycle_client(&db.pool, client_id, "online", true).await;

    let mut holder = db.pool.begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind("vpsman:operational-alert-reconcile")
        .execute(&mut *holder)
        .await
        .unwrap();
    let skipped = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        detect_offline_agents(&db.pool, 60),
    )
    .await
    .expect("offline sweep waited behind the reconcile advisory")
    .unwrap();
    assert_eq!(skipped, 0);
    assert_eq!(
        offline_side_effect_counts(&db.pool, client_id).await,
        (0, 0, 0, 0, 0)
    );

    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        sqlx::query(
            "UPDATE clients SET last_seen_at = clock_timestamp() - interval '2 hours' WHERE id = $1",
        )
        .bind(client_id)
        .execute(&db.pool),
    )
    .await
    .expect("offline sweep pinned the selected client row behind the advisory")
    .unwrap();

    holder.rollback().await.unwrap();
    assert_eq!(
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            detect_offline_agents(&db.pool, 60),
        )
        .await
        .expect("offline retry did not resume after the advisory was released")
        .unwrap(),
        1
    );
    assert_eq!(detect_offline_agents(&db.pool, 60).await.unwrap(), 0);
    assert_eq!(
        offline_side_effect_counts(&db.pool, client_id).await,
        (1, 1, 1, 1, 1)
    );

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_offline_candidate_plan_is_index_bounded_with_equal_timestamps() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    sqlx::query(
        r#"
        INSERT INTO clients (
            id, display_name, public_key, status, last_seen_at, capabilities
        )
        SELECT
            format('offline-plan-%s', lpad(client_number::text, 5, '0')),
            format('offline-plan-%s', lpad(client_number::text, 5, '0')),
            decode(md5(format('offline-plan-%s', client_number)), 'hex'),
            'online',
            date_trunc('minute', clock_timestamp() - interval '2 hours'),
            '{}'::jsonb
        FROM generate_series(1, 10000) client(client_number)
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query("ANALYZE clients")
        .execute(&db.pool)
        .await
        .unwrap();

    let explain_sql = format!("EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) {OFFLINE_CANDIDATE_SQL}");
    let plan: Value = sqlx::query_scalar(&explain_sql)
        .bind(60_f64)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let root = &plan[0]["Plan"];
    assert!(
        !plan_contains_node_type(root, "Sort"),
        "equal timestamps introduced a candidate sort: {plan}"
    );
    assert!(
        !relation_uses_node_type(root, "clients", "Seq Scan"),
        "offline candidate used a clients sequential scan: {plan}"
    );
    assert!(
        plan.to_string().contains("clients_visible_status_idx"),
        "offline candidate did not use the visible status/last-seen index: {plan}"
    );
    let examined = relation_examined_rows(root, "clients");
    assert!(
        examined <= 4.0,
        "offline candidate examined {examined} client rows: {plan}"
    );

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_offline_sweep_stops_at_batch_and_resumes_remaining_client() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    sqlx::query(
        r#"
        INSERT INTO clients (
            id, display_name, public_key, status, last_seen_at, capabilities
        )
        SELECT
            format('offline-batch-%s', lpad(client_number::text, 3, '0')),
            format('offline-batch-%s', lpad(client_number::text, 3, '0')),
            decode(md5(format('offline-batch-%s', client_number)), 'hex'),
            'online', clock_timestamp() - interval '2 hours', '{}'::jsonb
        FROM generate_series(1, 101) client(client_number)
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();

    assert_eq!(detect_offline_agents(&db.pool, 60).await.unwrap(), 100);
    let statuses: (i64, i64) = sqlx::query_as(
        r#"
        SELECT count(*) FILTER (WHERE status = 'offline'),
               count(*) FILTER (WHERE status = 'online')
        FROM clients
        "#,
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(statuses, (100, 1));
    assert_eq!(detect_offline_agents(&db.pool, 60).await.unwrap(), 1);
    assert_eq!(detect_offline_agents(&db.pool, 60).await.unwrap(), 0);
    let effects: (i64, i64, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT
            (SELECT count(*) FROM client_status_history
             WHERE from_status = 'online' AND to_status = 'offline'
               AND reason = 'agent_offline_timeout'),
            (SELECT count(*) FROM audit_logs WHERE action = 'agent.status_offline'),
            (SELECT count(*) FROM webhook_events WHERE kind = 'vps.status_changed'),
            (SELECT count(*) FROM alert_policy_evidence
             WHERE source_kind = 'agent.status' AND source_status = 'offline'),
            (SELECT count(*) FROM alert_policy_evidence
             WHERE source_kind = 'agent.access' AND source_status = 'offline')
        "#,
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(effects, (101, 101, 101, 101, 101));

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_offline_sweep_rolls_back_mid_transition_failure_before_retry() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    let client_id = "offline-sweep-rollback";
    insert_lifecycle_client(&db.pool, client_id, "online", true).await;
    sqlx::query(
        r#"
        CREATE FUNCTION fail_offline_history_for_test() RETURNS trigger
        LANGUAGE plpgsql AS $$
        BEGIN
            IF NEW.client_id = 'offline-sweep-rollback'
               AND NEW.reason = 'agent_offline_timeout' THEN
                RAISE EXCEPTION 'injected offline side-effect failure';
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
        CREATE TRIGGER fail_offline_history_for_test
        BEFORE INSERT ON client_status_history
        FOR EACH ROW EXECUTE FUNCTION fail_offline_history_for_test()
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();

    let error = detect_offline_agents(&db.pool, 60)
        .await
        .expect_err("injected side-effect failure unexpectedly committed");
    assert!(error
        .to_string()
        .contains("injected offline side-effect failure"));
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT status FROM clients WHERE id = $1")
            .bind(client_id)
            .fetch_one(&db.pool)
            .await
            .unwrap(),
        "online"
    );
    assert_eq!(
        offline_side_effect_counts(&db.pool, client_id).await,
        (0, 0, 0, 0, 0)
    );

    sqlx::query("DROP TRIGGER fail_offline_history_for_test ON client_status_history")
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("DROP FUNCTION fail_offline_history_for_test()")
        .execute(&db.pool)
        .await
        .unwrap();
    assert_eq!(detect_offline_agents(&db.pool, 60).await.unwrap(), 1);
    assert_eq!(detect_offline_agents(&db.pool, 60).await.unwrap(), 0);
    assert_eq!(
        offline_side_effect_counts(&db.pool, client_id).await,
        (1, 1, 1, 1, 1)
    );

    db.cleanup().await;
}

async fn offline_side_effect_counts(pool: &PgPool, client_id: &str) -> (i64, i64, i64, i64, i64) {
    sqlx::query_as(
        r#"
        SELECT
            (SELECT count(*) FROM client_status_history
             WHERE client_id = $1
               AND from_status = 'online' AND to_status = 'offline'
               AND reason = 'agent_offline_timeout'),
            (SELECT count(*) FROM audit_logs
             WHERE action = 'agent.status_offline'
               AND target = 'client:' || $1),
            (SELECT count(*) FROM webhook_events
             WHERE kind = 'vps.status_changed'
               AND $1 = ANY(subject_client_ids)),
            (SELECT count(*) FROM alert_policy_evidence
             WHERE subject_client_id = $1
               AND source_kind = 'agent.status'
               AND source_status = 'offline'),
            (SELECT count(*) FROM alert_policy_evidence
             WHERE subject_client_id = $1
               AND source_kind = 'agent.access'
               AND source_status = 'offline')
        "#,
    )
    .bind(client_id)
    .fetch_one(pool)
    .await
    .unwrap()
}

fn plan_contains_node_type(plan: &Value, node_type: &str) -> bool {
    plan.get("Node Type")
        .and_then(Value::as_str)
        .is_some_and(|candidate| candidate.contains(node_type))
        || plan
            .get("Plans")
            .and_then(Value::as_array)
            .is_some_and(|children| {
                children
                    .iter()
                    .any(|child| plan_contains_node_type(child, node_type))
            })
}

fn relation_uses_node_type(plan: &Value, relation: &str, node_type: &str) -> bool {
    (plan.get("Relation Name").and_then(Value::as_str) == Some(relation)
        && plan
            .get("Node Type")
            .and_then(Value::as_str)
            .is_some_and(|candidate| candidate.contains(node_type)))
        || plan
            .get("Plans")
            .and_then(Value::as_array)
            .is_some_and(|children| {
                children
                    .iter()
                    .any(|child| relation_uses_node_type(child, relation, node_type))
            })
}

fn relation_examined_rows(plan: &Value, relation: &str) -> f64 {
    let own = if plan.get("Relation Name").and_then(Value::as_str) == Some(relation) {
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
        let rechecked = plan
            .get("Rows Removed by Index Recheck")
            .and_then(Value::as_f64)
            .unwrap_or_default();
        (actual + filtered + rechecked) * loops
    } else {
        0.0
    };
    own + plan
        .get("Plans")
        .and_then(Value::as_array)
        .map(|children| {
            children
                .iter()
                .map(|child| relation_examined_rows(child, relation))
                .sum::<f64>()
        })
        .unwrap_or_default()
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
