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
