use super::*;
use crate::test_support::PgWorkerTestDb;
use chrono::Duration as ChronoDuration;
use vpsman_common::structurally_valid_projected_telemetry_tunnel;

fn tunnel_identities(
    metrics: &AgentMetrics,
    names: &[&str],
) -> std::collections::HashSet<ProjectedTelemetryTunnelIdentity> {
    metrics
        .tunnels
        .iter()
        .filter(|tunnel| names.contains(&tunnel.interface.as_str()))
        .filter_map(projected_telemetry_tunnel_identity)
        .collect()
}

fn managed_tunnel_interfaces(
    identities: &std::collections::HashSet<ProjectedTelemetryTunnelIdentity>,
) -> std::collections::HashSet<String> {
    identities
        .iter()
        .map(|identity| identity.interface.clone())
        .collect()
}

#[test]
fn webhook_rule_worker_config_clamps_operational_bounds_and_validates_retention() {
    assert_eq!(
        WebhookRuleWorkerConfig::new(0, 0, 1, 0, 0).unwrap(),
        WebhookRuleWorkerConfig {
            delivery_limit: 1,
            materialize_limit: 1,
            retention_days: 1,
            retention_prune_limit: 1,
            webhook_timeout_secs: 1,
        }
    );
    assert_eq!(
        WebhookRuleWorkerConfig::new(10_000, 10_000, 3_650, 20_000, 120).unwrap(),
        WebhookRuleWorkerConfig {
            delivery_limit: 200,
            materialize_limit: 1000,
            retention_days: 3_650,
            retention_prune_limit: 10_000,
            webhook_timeout_secs: 60,
        }
    );
    assert!(WebhookRuleWorkerConfig::new(25, 100, 0, 1_000, 5).is_err());
    assert!(WebhookRuleWorkerConfig::new(25, 100, 3_651, 1_000, 5).is_err());
}

#[test]
fn telemetry_webhook_interface_projection_filters_bytes_but_preserves_tunnel_operation() {
    let metrics = AgentMetrics {
        networks: vec![
            vpsman_common::NetworkStat {
                interface: "eth0".to_string(),
                rx_bytes: 10,
                tx_bytes: 20,
            },
            vpsman_common::NetworkStat {
                interface: "docker0".to_string(),
                rx_bytes: 30,
                tx_bytes: 40,
            },
            vpsman_common::NetworkStat {
                interface: "wg0".to_string(),
                rx_bytes: 50,
                tx_bytes: 60,
            },
            vpsman_common::NetworkStat {
                interface: "wan0".to_string(),
                rx_bytes: 70,
                tx_bytes: 80,
            },
        ],
        tunnels: vec![
            vpsman_common::RuntimeTunnelStat {
                interface: "wg0".to_string(),
                kind: "wireguard".to_string(),
                operstate: Some("up".to_string()),
                rx_bytes: 70,
                tx_bytes: 80,
                traffic_status: Some("ok".to_string()),
                ..Default::default()
            },
            vpsman_common::RuntimeTunnelStat {
                interface: "tun0".to_string(),
                kind: "gre".to_string(),
                operstate: Some("down".to_string()),
                rx_bytes: 90,
                tx_bytes: 100,
                ..Default::default()
            },
            vpsman_common::RuntimeTunnelStat {
                interface: "wan0".to_string(),
                kind: "wireguard".to_string(),
                operstate: Some("up".to_string()),
                plan_id: Some("11111111-1111-4111-8111-111111111111".to_string()),
                plan_name: Some("managed-wan".to_string()),
                endpoint_side: Some("left".to_string()),
                peer_client_id: Some("managed-wan-peer".to_string()),
                rx_bytes: 110,
                tx_bytes: 120,
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    assert!(!structurally_valid_projected_telemetry_tunnel(
        &metrics.tunnels[0]
    ));
    assert!(structurally_valid_projected_telemetry_tunnel(
        &metrics.tunnels[2]
    ));
    let current_tunnel_identities = tunnel_identities(&metrics, &["wan0"]);

    let default = telemetry_webhook_interfaces(
        &metrics,
        &NetworkInterfacePolicy::DefaultPhysical,
        &[0b0000_1111],
        &[0b0000_0111],
        &current_tunnel_identities,
        &managed_tunnel_interfaces(&current_tunnel_identities),
    )
    .unwrap();
    assert_eq!(
        default
            .networks
            .iter()
            .map(|network| network.interface.as_str())
            .collect::<Vec<_>>(),
        vec!["eth0", "wg0"]
    );
    assert_eq!(default.tunnels.len(), 3);
    assert_eq!(default.tunnels[0]["interface"], "wg0");
    assert_eq!(default.tunnels[0]["operstate"], "up");
    assert_eq!(default.tunnels[1]["operstate"], "down");
    for (projected, source) in default.tunnels.iter().zip(&metrics.tunnels) {
        let mut expected = serde_json::to_value(source).unwrap();
        {
            let object = expected.as_object_mut().unwrap();
            for field in [
                "rx_bytes",
                "tx_bytes",
                "traffic_source",
                "traffic_status",
                "traffic_reason",
                "traffic_checked_unix",
            ] {
                object.remove(field);
            }
        }
        assert_eq!(projected, &expected);
        let tunnel = projected;
        assert!(tunnel.get("rx_bytes").is_none());
        assert!(tunnel.get("tx_bytes").is_none());
        assert!(tunnel.get("traffic_status").is_none());
    }

    let selected = telemetry_webhook_interfaces(
        &metrics,
        &NetworkInterfacePolicy::Patterns(vec!["docker0".to_string(), "wg*".to_string()]),
        &[0b0000_1111],
        &[0b0000_0111],
        &current_tunnel_identities,
        &managed_tunnel_interfaces(&current_tunnel_identities),
    )
    .unwrap();
    assert_eq!(
        selected
            .networks
            .iter()
            .map(|network| network.interface.as_str())
            .collect::<Vec<_>>(),
        vec!["docker0", "wg0"]
    );
    assert!(selected.tunnels[0].get("rx_bytes").is_none());
    assert!(selected.tunnels[0].get("tx_bytes").is_none());
    assert!(selected.tunnels[1].get("rx_bytes").is_none());
    assert_eq!(selected.tunnels[1]["operstate"], "down");

    let all = telemetry_webhook_interfaces(
        &metrics,
        &NetworkInterfacePolicy::All,
        &[0b0000_1111],
        &[0b0000_0111],
        &current_tunnel_identities,
        &managed_tunnel_interfaces(&current_tunnel_identities),
    )
    .unwrap();
    assert_eq!(all.networks.len(), 4);
    assert!(all.tunnels[0].get("rx_bytes").is_none());
    assert!(all.tunnels[1].get("rx_bytes").is_none());
    assert_eq!(all.tunnels[2]["rx_bytes"], 110);

    let mut replaced_identity = metrics.clone();
    replaced_identity.tunnels[2].plan_id = Some(Uuid::new_v4().to_string());
    let mismatched = telemetry_webhook_interfaces(
        &replaced_identity,
        &NetworkInterfacePolicy::All,
        &[0b0000_1111],
        &[0b0000_0111],
        &current_tunnel_identities,
        &managed_tunnel_interfaces(&current_tunnel_identities),
    )
    .unwrap();
    assert!(mismatched.tunnels[2].get("rx_bytes").is_none());

    // A missing exact runtime identity cannot expose tunnel bytes, while the
    // plan-owned interface still suppresses the same-named default host.
    let stale = telemetry_webhook_interfaces(
        &metrics,
        &NetworkInterfacePolicy::DefaultPhysical,
        &[0b0000_1111],
        &[0b0000_0111],
        &std::collections::HashSet::new(),
        &std::collections::HashSet::from(["wan0".to_string()]),
    )
    .unwrap();
    assert_eq!(
        stale
            .networks
            .iter()
            .map(|network| network.interface.as_str())
            .collect::<Vec<_>>(),
        vec!["eth0", "wg0"]
    );
    assert!(stale
        .tunnels
        .iter()
        .all(|tunnel| tunnel.get("rx_bytes").is_none()));
}

#[test]
fn telemetry_webhook_projection_rejects_entire_nine_entry_vector_for_non_exact_mask() {
    let metrics = AgentMetrics {
        networks: (0..9)
            .map(|ordinal| vpsman_common::NetworkStat {
                interface: format!("eth{ordinal}"),
                rx_bytes: ordinal,
                tx_bytes: ordinal + 100,
            })
            .collect(),
        tunnels: (0..9)
            .map(|ordinal| vpsman_common::RuntimeTunnelStat {
                interface: format!("wg{ordinal}"),
                kind: "wireguard".to_string(),
                operstate: Some("up".to_string()),
                plan_id: Some(Uuid::from_u128(ordinal as u128 + 1).to_string()),
                plan_name: Some(format!("managed-{ordinal}")),
                endpoint_side: Some("left".to_string()),
                peer_client_id: Some(format!("peer-{ordinal}")),
                rx_bytes: ordinal,
                tx_bytes: ordinal + 100,
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    };
    let current_names = metrics
        .tunnels
        .iter()
        .map(|tunnel| tunnel.interface.as_str())
        .collect::<Vec<_>>();
    let current_tunnel_identities = tunnel_identities(&metrics, &current_names);

    for malformed_mask in [
        &[0xff][..],
        &[0xff, 0b0000_0010][..],
        &[0xff, 0b0000_0001, 0][..],
    ] {
        let projected = telemetry_webhook_interfaces(
            &metrics,
            &NetworkInterfacePolicy::All,
            malformed_mask,
            malformed_mask,
            &current_tunnel_identities,
            &managed_tunnel_interfaces(&current_tunnel_identities),
        )
        .unwrap();
        assert!(projected.networks.is_empty());
        assert_eq!(projected.tunnels.len(), 9);
        assert!(projected
            .tunnels
            .iter()
            .all(|tunnel| tunnel.get("rx_bytes").is_none() && tunnel.get("tx_bytes").is_none()));
    }

    let exact = telemetry_webhook_interfaces(
        &metrics,
        &NetworkInterfacePolicy::All,
        &[0xff, 0b0000_0001],
        &[0xff, 0b0000_0001],
        &current_tunnel_identities,
        &managed_tunnel_interfaces(&current_tunnel_identities),
    )
    .unwrap();
    assert_eq!(exact.networks.len(), 9);
    assert!(exact
        .tunnels
        .iter()
        .all(|tunnel| tunnel.get("rx_bytes").is_some() && tunnel.get("tx_bytes").is_some()));
}

#[tokio::test]
async fn postgres_ordinal_admission_mask_shape_rejects_short_and_unused_high_bits() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    assert_eq!(
        sqlx::query_as::<_, (bool, bool, bool, bool, bool)>(
            r#"
            SELECT
                public.telemetry_ordinal_admission_mask_is_exact(
                    decode('ff', 'hex'), 9
                ),
                public.telemetry_ordinal_admission_mask_is_exact(
                    decode('ff01', 'hex'), 9
                ),
                public.telemetry_ordinal_admission_mask_is_exact(
                    decode('ff02', 'hex'), 9
                ),
                public.telemetry_ordinal_admission_mask_is_exact(
                    decode('ff0100', 'hex'), 9
                ),
                public.telemetry_ordinal_admission_mask_is_exact(
                    '\x'::BYTEA, 0
                )
            "#,
        )
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        (false, true, false, false, true)
    );
    db.cleanup().await;
}

#[test]
fn telemetry_webhook_projection_intersects_non_byte_aligned_stamp_with_current_policy() {
    let metrics = AgentMetrics {
        networks: (0..10)
            .map(|ordinal| vpsman_common::NetworkStat {
                interface: if ordinal == 1 {
                    "docker0".to_string()
                } else {
                    format!("eth{ordinal}")
                },
                rx_bytes: ordinal,
                tx_bytes: ordinal + 100,
            })
            .collect(),
        tunnels: (0..10)
            .map(|ordinal| vpsman_common::RuntimeTunnelStat {
                interface: if ordinal == 1 {
                    "tun0".to_string()
                } else {
                    format!("wg{ordinal}")
                },
                kind: "wireguard".to_string(),
                operstate: Some("up".to_string()),
                plan_id: Some(Uuid::from_u128(ordinal as u128 + 100).to_string()),
                plan_name: Some(format!("managed-{ordinal}")),
                endpoint_side: Some("left".to_string()),
                peer_client_id: Some(format!("peer-{ordinal}")),
                rx_bytes: ordinal,
                tx_bytes: ordinal + 100,
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    };
    let current_names = metrics
        .tunnels
        .iter()
        .map(|tunnel| tunnel.interface.as_str())
        .collect::<Vec<_>>();
    let current_tunnel_identities = tunnel_identities(&metrics, &current_names);
    let projected = telemetry_webhook_interfaces(
        &metrics,
        &NetworkInterfacePolicy::Patterns(vec!["eth*".to_string(), "wg*".to_string()]),
        // Projection admitted eth0 + docker0 but rejected currently selected
        // eth8. Current policy narrows docker0 and cannot widen eth8.
        &[0b0000_0011, 0],
        // Projection admitted tun0 + ordinal-eight wg8. Current policy narrows
        // tun0 and admits wg8 across the partial final byte.
        &[0b0000_0010, 0b0000_0001],
        &current_tunnel_identities,
        &managed_tunnel_interfaces(&current_tunnel_identities),
    )
    .unwrap();

    assert_eq!(
        projected
            .networks
            .iter()
            .map(|network| network.interface.as_str())
            .collect::<Vec<_>>(),
        vec!["eth0"]
    );
    assert_eq!(projected.tunnels.len(), 10);
    assert!(projected.tunnels[0].get("rx_bytes").is_none());
    assert!(projected.tunnels[1].get("rx_bytes").is_none());
    assert_eq!(projected.tunnels[8]["rx_bytes"], 8);
    assert_eq!(projected.tunnels[8]["operstate"], "up");
    assert!(projected.tunnels[9].get("rx_bytes").is_none());
}

#[test]
fn telemetry_webhook_source_requires_both_non_null_projection_masks_without_fallback() {
    let source = include_str!("webhook_rules.rs");
    let materializer = source
        .split_once("async fn process_telemetry_projection_events")
        .unwrap()
        .1
        .split_once("fn telemetry_event_from_projection_row")
        .unwrap()
        .0;
    assert!(materializer.contains("JOIN telemetry_samples sample"));
    assert!(materializer.contains("sample.accepted_seq > head.initial_cursor_seq"));
    assert!(materializer.contains("sample.network_admission_mask"));
    assert!(materializer.contains("sample.tunnel_admission_mask"));
    assert!(materializer.contains("FROM telemetry_current_tunnels tunnel"));
    assert!(materializer.contains("AS current_tunnel_identities"));
    assert!(materializer.contains("managed_tunnel_interfaces AS MATERIALIZED"));
    assert!(materializer.contains("plan.left_client_id AS client_id"));
    assert!(materializer.contains("plan.right_client_id AS client_id"));
    assert!(!materializer.contains("COALESCE(sample.network_admission_mask"));
    assert!(!materializer.contains("COALESCE(sample.tunnel_admission_mask"));

    let builder = source
        .split_once("fn telemetry_event_from_projection_row")
        .unwrap()
        .1
        .split_once("pub(crate) async fn process_webhook_events")
        .unwrap()
        .0;
    assert!(builder.contains("row.try_get(\"network_admission_mask\")"));
    assert!(builder.contains("row.try_get(\"tunnel_admission_mask\")"));
    assert!(builder.contains("current_tunnel_identities\""));
    assert!(builder.contains("managed_tunnel_interfaces\""));
    assert!(builder.contains("ordinal_admission_mask_has_exact_shape(network_admission_mask"));
    assert!(builder.contains("ordinal_admission_mask_has_exact_shape(tunnel_admission_mask"));
    assert!(builder.contains("ordinal_admitted(network_admission_mask"));
    assert!(builder.contains("ordinal_admitted(tunnel_admission_mask"));
    assert!(builder.contains("managed_tunnel_interfaces.contains(&network.interface)"));
    assert!(builder.contains("projected_telemetry_tunnel_identity(tunnel)"));
    assert!(builder.contains("current_tunnel_identities.contains(&identity)"));
    assert!(!builder.contains("structurally_valid_projected_telemetry_tunnel"));
}

#[test]
fn telemetry_transaction_batch_is_not_a_per_tick_throughput_cap() {
    let source = include_str!("webhook_rules.rs");
    let (_, work_pending) = source
        .split_once("async fn webhook_event_materialization_pending")
        .expect("webhook work-pending probe");
    let (work_pending, _) = work_pending
        .split_once("async fn drain_webhook_retention")
        .expect("webhook work-pending probe boundary");
    assert!(work_pending.contains("FROM webhook_events WHERE processed_at IS NULL"));
    assert!(!work_pending.contains("telemetry_webhook_cursors"));

    let (_, drain) = source
        .split_once("async fn drain_telemetry_projection_events")
        .expect("telemetry projection drain");
    let (drain, _) = drain
        .split_once("async fn project_alert_lifecycle_events")
        .expect("telemetry projection drain boundary");
    assert!(drain.contains("loop {"));
    assert!(drain.contains("process_telemetry_projection_events(pool, config).await?"));
    assert!(drain.contains("FROM telemetry_webhook_cursors cursor"));
    assert!(drain.contains("cursor.last_sample_seq < head.projected_seq"));
    assert!(!drain.contains("cursor.last_sample_seq < head.accepted_seq"));
    assert!(drain.contains("tokio::task::yield_now().await"));

    let worker = include_str!("../main.rs");
    let (_, listener) = worker
        .split_once("async fn connect_worker_notification_listener")
        .expect("worker notification listener");
    let (listener, _) = listener
        .split_once("async fn process_alert_notification_work")
        .expect("webhook listener boundary");
    assert!(listener.contains(".listen(\"webhook_events\")"));
    assert!(listener.contains(".listen(\"vpsman_telemetry_projection\")"));
    assert!(listener.contains(".listen(\"vpsman_telemetry_retention\")"));
    assert!(listener.contains(".listen(ARTIFACT_DELETION_COMPLETED_CHANNEL)"));
}

#[test]
fn telemetry_projection_wake_has_one_bounded_owner_and_exact_no_rule_seek() {
    let source = include_str!("webhook_rules.rs");
    let (_, telemetry_wake) = source
        .split_once("pub(crate) async fn process_telemetry_webhook_materialization_work")
        .expect("telemetry webhook wake owner");
    let (telemetry_wake, _) = telemetry_wake
        .split_once("pub(crate) async fn process_webhook_event_materialization_work")
        .expect("telemetry webhook wake owner boundary");
    assert!(telemetry_wake.contains("drain_telemetry_projection_without_enabled_rules_for_clients"));
    assert!(telemetry_wake.contains("drain_telemetry_projection_events"));
    assert!(!telemetry_wake.contains("process_queued_deliveries"));
    for unrelated_path in [
        "project_alert_lifecycle_events",
        "process_webhook_events",
        "webhook_event_materialization_pending",
        "materialize_interval_events",
        "drain_webhook_retention",
        "prune_webhook_events",
    ] {
        assert!(!telemetry_wake.contains(unrelated_path), "{unrelated_path}");
    }

    let (_, exact_no_rule) = source
        .split_once("async fn drain_telemetry_projection_without_enabled_rules_for_clients")
        .expect("exact telemetry no-rule owner");
    let (exact_no_rule, _) = exact_no_rule
        .split_once("async fn drain_telemetry_projection_events")
        .expect("exact telemetry no-rule owner boundary");
    assert!(exact_no_rule.contains("cursor.client_id = ANY($2::TEXT[])"));
    assert!(exact_no_rule.contains("cursor.client_id = ANY($1::TEXT[])"));
    assert!(exact_no_rule.contains("FOR UPDATE OF cursor SKIP LOCKED"));
    assert!(exact_no_rule.contains("loop {"));
    assert!(exact_no_rule.contains("tokio::task::yield_now().await"));
    assert!(!exact_no_rule.contains("UPDATE telemetry_projection_heads"));
    assert!(!exact_no_rule.contains("LOCK TABLE"));
}

#[test]
fn no_rule_telemetry_cursor_is_one_statement_before_repeatable_read_materialization() {
    let source = include_str!("webhook_rules.rs");
    let (_, no_rule) = source
        .split_once("async fn try_advance_telemetry_webhook_cursor_without_enabled_rules")
        .expect("no-rule telemetry cursor path");
    let (no_rule, materialization) = no_rule
        .split_once("async fn process_telemetry_projection_events")
        .expect("enabled-rule telemetry materialization path");
    assert!(no_rule.contains("WITH configuration AS MATERIALIZED"));
    assert!(no_rule.contains("CROSS JOIN configuration"));
    assert!(no_rule.contains("FOR UPDATE OF cursor SKIP LOCKED"));
    assert!(no_rule.contains("UPDATE telemetry_webhook_cursors cursor"));
    assert!(no_rule.contains("cursor.last_sample_seq < head.projected_seq"));
    assert!(no_rule.contains("SET last_sample_seq = source.projected_seq"));
    assert!(no_rule.contains("source.projected_seq <= head.projected_seq"));
    assert!(!no_rule.contains("cursor.last_sample_seq < head.accepted_seq"));
    assert!(!no_rule.contains("SET last_sample_seq = source.accepted_seq"));
    assert!(!no_rule.contains("UPDATE telemetry_projection_heads"));
    assert!(no_rule.contains(".fetch_one(pool)"));
    assert!(!no_rule.contains("LOCK TABLE"));
    assert!(!no_rule.contains("pool.begin()"));
    assert!(!no_rule.contains("REPEATABLE READ"));
    assert!(materialization.contains("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ"));
}

#[test]
fn telemetry_cursor_wakes_sample_prune_only_for_an_already_due_first_row() {
    let source = include_str!("webhook_rules.rs");
    assert_eq!(
        source
            .matches("'effect', 'sample_prune_frontier_advanced'")
            .count(),
        4,
        "three cursor statements and the enabled-rule transaction share one typed effect"
    );
    assert_eq!(source.matches("WHERE sample_prune_due\n").count(), 3);
    assert!(source.contains("first_sample.accepted_seq = candidate.last_sample_seq + 1"));
    assert!(source.contains(
        "first_sample.observed_at\n                            < now() - make_interval(days => $3)"
    ));
    assert!(source.contains(
        "first_sample.observed_at\n                        < now() - make_interval(days => $2)"
    ));

    let (_, enabled) = source
        .split_once("async fn process_telemetry_projection_events")
        .expect("enabled telemetry webhook cursor path");
    let (enabled, _) = enabled
        .split_once("fn telemetry_event_from_projection_row")
        .expect("enabled telemetry webhook cursor boundary");
    assert!(enabled.contains("sample.accepted_seq = head.initial_cursor_seq + 1"));
    assert!(enabled.contains("AS sample_prune_due"));
    assert!(enabled
        .contains("if sample_prune_due {\n        notify_sample_prune_frontier_advanced_in_tx"));
}

#[test]
fn telemetry_transaction_source_rows_derive_from_configured_materialization_bound() {
    assert_eq!(telemetry_webhook_source_rows(0, 100), 100);
    assert_eq!(telemetry_webhook_source_rows(1, 100), 100);
    assert_eq!(telemetry_webhook_source_rows(2, 100), 50);
    assert_eq!(telemetry_webhook_source_rows(8, 100), 12);
    assert_eq!(telemetry_webhook_source_rows(100, 100), 1);
    assert_eq!(telemetry_webhook_source_rows(1_000, 100), 1);
    assert_eq!(telemetry_webhook_source_rows(2, 1_000), 500);
}

#[tokio::test]
async fn postgres_event_owner_executes_without_periodic_maintenance() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    assert_eq!(
        process_webhook_event_materialization_work(&db.pool, WebhookRuleWorkerConfig::default())
            .await
            .unwrap(),
        WebhookRuleWorkerRun::default()
    );
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_exact_no_rule_wake_advances_only_notified_clients() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    let client_a = "telemetry-exact-wake-a";
    let client_b = "telemetry-exact-wake-b";
    for client_id in [client_a, client_b] {
        insert_webhook_test_client(&db.pool, client_id, "online", false).await;
        insert_telemetry_projection_sample(
            &db.pool,
            client_id,
            1,
            Utc::now(),
            "gateway-exact-wake",
            Uuid::new_v4(),
            Uuid::new_v4(),
            1,
            23_000,
            &AgentMetrics {
                observed_unix: 23_000,
                hostname: client_id.to_string(),
                ..Default::default()
            },
        )
        .await;
    }

    assert_eq!(
        process_telemetry_webhook_materialization_work(
            &db.pool,
            WebhookRuleWorkerConfig::default(),
            &[client_a.to_string()],
        )
        .await
        .unwrap(),
        WebhookRuleWorkerRun::default()
    );
    assert_eq!(
        sqlx::query_as::<_, (i64, i64)>(
            r#"
            SELECT
                (SELECT last_sample_seq FROM telemetry_webhook_cursors WHERE client_id=$1),
                (SELECT last_sample_seq FROM telemetry_webhook_cursors WHERE client_id=$2)
            "#,
        )
        .bind(client_a)
        .bind(client_b)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        (1, 0),
        "an exact notification must not scan or acknowledge another client"
    );

    process_telemetry_webhook_materialization_work(
        &db.pool,
        WebhookRuleWorkerConfig::default(),
        &[client_b.to_string()],
    )
    .await
    .unwrap();
    assert_eq!(
        sqlx::query_as::<_, (i64, i64)>(
            r#"
            SELECT
                (SELECT last_sample_seq FROM telemetry_webhook_cursors WHERE client_id=$1),
                (SELECT last_sample_seq FROM telemetry_webhook_cursors WHERE client_id=$2)
            "#,
        )
        .bind(client_a)
        .bind(client_b)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        (1, 1),
        "every separately notified client remains drainable"
    );
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_enabled_telemetry_wake_materializes_before_independent_delivery() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    let client_id = "telemetry-enabled-wake";
    insert_webhook_test_client(&db.pool, client_id, "online", false).await;
    let rule_id =
        insert_webhook_test_rule(&db.pool, "telemetry enabled wake", "telemetry.rollup").await;
    insert_telemetry_projection_sample(
        &db.pool,
        client_id,
        1,
        Utc::now(),
        "gateway-enabled-wake",
        Uuid::new_v4(),
        Uuid::new_v4(),
        1,
        24_000,
        &AgentMetrics {
            observed_unix: 24_000,
            hostname: client_id.to_string(),
            ..Default::default()
        },
    )
    .await;

    let run = process_telemetry_webhook_materialization_work(
        &db.pool,
        WebhookRuleWorkerConfig::default(),
        &[client_id.to_string()],
    )
    .await
    .unwrap();
    assert_eq!(run.materialized, 1);
    assert_eq!((run.processed, run.delivered, run.failed), (0, 0, 0));
    assert_eq!(
        sqlx::query_as::<_, (String, i32, i64)>(
            r#"
            SELECT status, attempt_count,
                   (SELECT last_sample_seq
                    FROM telemetry_webhook_cursors
                    WHERE client_id=$2)
            FROM webhook_rule_deliveries
            WHERE rule_id=$1
              AND event_kind='telemetry.rollup'
            "#,
        )
        .bind(rule_id)
        .bind(client_id)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        ("queued".to_string(), 0, 1),
        "materialization commits the delivery without awaiting external I/O"
    );
    let delivery = process_due_webhook_deliveries(&db.pool, WebhookRuleWorkerConfig::default())
        .await
        .unwrap();
    assert_eq!(
        (delivery.processed, delivery.delivered, delivery.failed),
        (1, 0, 1)
    );
    assert_eq!(
        sqlx::query_as::<_, (String, i32)>(
            "SELECT status, attempt_count FROM webhook_rule_deliveries WHERE rule_id=$1",
        )
        .bind(rule_id)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        ("permanently_failed".to_string(), 1),
        "the leased delivery consumer owns the HTTP attempt"
    );
    assert_eq!(
        process_telemetry_webhook_materialization_work(
            &db.pool,
            WebhookRuleWorkerConfig::default(),
            &[client_id.to_string()],
        )
        .await
        .unwrap(),
        WebhookRuleWorkerRun::default(),
        "a completed cursor and terminal delivery remain idempotent"
    );
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_projection_and_webhook_cursors_have_independent_locks_over_one_raw_journal() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    let client_id = "telemetry-cursor-owner-lock-edge";
    insert_webhook_test_client(&db.pool, client_id, "online", false).await;
    insert_telemetry_projection_sample(
        &db.pool,
        client_id,
        1,
        Utc::now(),
        "gateway-cursor-owner-lock",
        Uuid::new_v4(),
        Uuid::new_v4(),
        1,
        22_000,
        &AgentMetrics {
            observed_unix: 22_001,
            hostname: client_id.to_string(),
            ..Default::default()
        },
    )
    .await;
    sqlx::query(
        "UPDATE telemetry_projection_heads SET projected_seq=0, projected_at=NULL WHERE client_id=$1",
    )
    .bind(client_id)
    .execute(&db.pool)
    .await
    .unwrap();

    let mut webhook_owner = db.pool.begin().await.unwrap();
    sqlx::query_scalar::<_, i64>(
        "SELECT last_sample_seq FROM telemetry_webhook_cursors WHERE client_id=$1 FOR UPDATE",
    )
    .bind(client_id)
    .fetch_one(&mut *webhook_owner)
    .await
    .unwrap();
    let mut projection_owner = db.pool.begin().await.unwrap();
    sqlx::query("SET LOCAL lock_timeout = '250ms'")
        .execute(&mut *projection_owner)
        .await
        .unwrap();
    sqlx::query("UPDATE telemetry_projection_heads SET projected_seq=1 WHERE client_id=$1")
        .bind(client_id)
        .execute(&mut *projection_owner)
        .await
        .expect("webhook ownership must not block the independent projector cursor");
    projection_owner.commit().await.unwrap();
    webhook_owner.rollback().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM telemetry_samples WHERE client_id=$1",)
            .bind(client_id)
            .fetch_one(&db.pool)
            .await
            .unwrap(),
        1,
        "the durable raw source remains while either consumer is pending"
    );

    let mut projection_owner = db.pool.begin().await.unwrap();
    sqlx::query_scalar::<_, i64>(
        "SELECT projected_seq FROM telemetry_projection_heads WHERE client_id=$1 FOR UPDATE",
    )
    .bind(client_id)
    .fetch_one(&mut *projection_owner)
    .await
    .unwrap();
    let mut webhook_owner = db.pool.begin().await.unwrap();
    sqlx::query("SET LOCAL lock_timeout = '250ms'")
        .execute(&mut *webhook_owner)
        .await
        .unwrap();
    sqlx::query("UPDATE telemetry_webhook_cursors SET last_sample_seq=1 WHERE client_id=$1")
        .bind(client_id)
        .execute(&mut *webhook_owner)
        .await
        .expect("projection ownership must not block the independent webhook cursor");
    webhook_owner.commit().await.unwrap();
    projection_owner.rollback().await.unwrap();
    assert_eq!(
        sqlx::query_as::<_, (i64, i64, i64)>(
            r#"
            SELECT head.accepted_seq, head.projected_seq, cursor.last_sample_seq
            FROM telemetry_projection_heads head
            JOIN telemetry_webhook_cursors cursor USING (client_id)
            WHERE head.client_id=$1
            "#,
        )
        .bind(client_id)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        (1, 1, 1)
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM telemetry_samples WHERE client_id=$1",)
            .bind(client_id)
            .fetch_one(&db.pool)
            .await
            .unwrap(),
        1,
        "consumer advancement must not prune bounded raw history"
    );

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_enabled_telemetry_waits_for_projection_then_materializes_exact_delivery() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    let client_id = "telemetry-cursor-direct-edge";
    insert_webhook_test_client(&db.pool, client_id, "online", false).await;
    let rule_id = insert_webhook_test_rule(
        &db.pool,
        "telemetry cursor delivery",
        "telemetry.rollup && telemetry.network_count = 2",
    )
    .await;
    let gateway_session_id = Uuid::new_v4();
    let process_incarnation_id = Uuid::new_v4();
    let accepted_at = DateTime::<Utc>::from_timestamp(20_000, 0).unwrap();
    let metrics = AgentMetrics {
        observed_unix: 30_000,
        hostname: "source-hostname".to_string(),
        uptime_secs: 321,
        disks: vec![vpsman_common::DiskStat {
            mountpoint: "/".to_string(),
            total_bytes: 1_000,
            available_bytes: 400,
        }],
        disk_collection_available: Some(true),
        disk_semantics: Some(
            vpsman_common::DISK_SEMANTICS_PERSISTENT_BLOCK_FILESYSTEMS_V1.to_string(),
        ),
        networks: vec![
            vpsman_common::NetworkStat {
                interface: "eth0".to_string(),
                rx_bytes: 100,
                tx_bytes: 200,
            },
            vpsman_common::NetworkStat {
                interface: "eth1".to_string(),
                rx_bytes: 300,
                tx_bytes: 400,
            },
        ],
        tunnels: vec![vpsman_common::RuntimeTunnelStat {
            interface: "wg0".to_string(),
            rx_bytes: 50,
            tx_bytes: 60,
            ..Default::default()
        }],
        ..Default::default()
    };
    insert_telemetry_projection_sample(
        &db.pool,
        client_id,
        1,
        accepted_at,
        "gateway-cursor-direct",
        gateway_session_id,
        process_incarnation_id,
        41,
        19_000,
        &metrics,
    )
    .await;
    sqlx::query(
        r#"
        UPDATE telemetry_projection_heads
        SET projected_seq=0, projected_at=NULL
        WHERE client_id=$1
        "#,
    )
    .bind(client_id)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        UPDATE telemetry_samples
        SET network_admission_mask='\x'::BYTEA,
            tunnel_admission_mask='\x'::BYTEA
        WHERE client_id=$1 AND accepted_seq=1
        "#,
    )
    .bind(client_id)
    .execute(&db.pool)
    .await
    .unwrap();
    // Build the equivalent canonical outbox event independently. The direct
    // cursor uses canonical acceptance time, while every public event and
    // delivery field for that occurrence must remain identical.
    let reference_event = telemetry_event_reference(
        client_id,
        "gateway-cursor-direct",
        gateway_session_id,
        process_incarnation_id,
        41,
        19_000,
        accepted_at,
        &metrics,
    );
    let rules = list_enabled_rules(&db.pool, 10).await.unwrap();
    let rule = rules.iter().find(|rule| rule.id == rule_id).unwrap();
    let mut reference_tx = db.pool.begin().await.unwrap();
    let reference_vps = list_event_vps(&mut reference_tx, false, &[client_id.to_string()])
        .await
        .unwrap();
    reference_tx.rollback().await.unwrap();
    let reference_delivery = event_candidate_for_rule(rule, &reference_event, &reference_vps)
        .unwrap()
        .expect("the reference telemetry event must match the enabled rule");

    let event_id =
        format!("telemetry:{client_id}:{gateway_session_id}:{process_incarnation_id}:41");
    assert_eq!(
        tokio::time::timeout(
            Duration::from_secs(2),
            drain_telemetry_projection_events(&db.pool, WebhookRuleWorkerConfig::default()),
        )
        .await
        .expect("accepted-but-unprojected telemetry must not keep the drain spinning")
        .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_as::<_, (i64, i64)>(
            r#"
            SELECT cursor.last_sample_seq, count(delivery.id)::BIGINT
            FROM telemetry_webhook_cursors cursor
            LEFT JOIN webhook_rule_deliveries delivery ON delivery.event_id=$2
            WHERE cursor.client_id=$1
            GROUP BY cursor.last_sample_seq
            "#,
        )
        .bind(client_id)
        .bind(&event_id)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        (0, 0),
        "enabled webhooks must not observe or acknowledge the unstamped suffix"
    );

    let mut projection = db.pool.begin().await.unwrap();
    sqlx::query(
        r#"
        UPDATE telemetry_samples
        SET network_admission_mask='\x03'::BYTEA,
            tunnel_admission_mask='\x00'::BYTEA
        WHERE client_id=$1 AND accepted_seq=1
        "#,
    )
    .bind(client_id)
    .execute(&mut *projection)
    .await
    .unwrap();
    sqlx::query(
        r#"
        UPDATE telemetry_projection_heads
        SET projected_seq=1, projected_at=clock_timestamp()
        WHERE client_id=$1 AND projected_seq=0
        "#,
    )
    .bind(client_id)
    .execute(&mut *projection)
    .await
    .unwrap();
    projection.commit().await.unwrap();

    assert_eq!(
        process_telemetry_projection_events(&db.pool, WebhookRuleWorkerConfig::default())
            .await
            .unwrap(),
        1
    );
    let delivery = sqlx::query(
        r#"
        SELECT rule_id, event_kind, event_id, status, attempt_count, target,
               dedupe_key, payload, matched_vps, message, cooldown_until_unix
        FROM webhook_rule_deliveries
        WHERE event_id = $1
        "#,
    )
    .bind(&event_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        delivery.get::<Uuid, _>("rule_id"),
        reference_delivery.rule_id
    );
    assert_eq!(
        delivery.get::<String, _>("event_kind"),
        reference_delivery.event_kind
    );
    assert_eq!(
        delivery.get::<String, _>("event_id"),
        reference_delivery.event_id
    );
    assert_eq!(delivery.get::<String, _>("status"), "queued");
    assert_eq!(delivery.get::<i32, _>("attempt_count"), 0);
    assert_eq!(
        delivery.get::<String, _>("target"),
        reference_delivery.target
    );
    assert_eq!(
        delivery.get::<String, _>("dedupe_key"),
        reference_delivery.dedupe_key
    );
    assert_eq!(
        delivery.get::<String, _>("message"),
        reference_delivery.message
    );
    assert_eq!(
        delivery.get::<i64, _>("cooldown_until_unix"),
        reference_delivery.cooldown_until_unix
    );
    assert_eq!(
        delivery.get::<SqlJson<Value>, _>("matched_vps").0,
        serde_json::to_value(&reference_delivery.matched_vps).unwrap()
    );
    let payload = delivery.get::<SqlJson<Value>, _>("payload").0;
    assert_eq!(payload, reference_delivery.payload);
    assert_eq!(payload["event"]["occurred_at_unix"], 20_000);
    assert_eq!(
        payload["event"]["predicates"],
        json!([
            "telemetry.network_rate",
            "telemetry.rollup",
            "telemetry.tunnel"
        ])
    );
    assert_eq!(payload["telemetry"]["client_id"], client_id);
    assert_eq!(payload["telemetry"]["gateway_id"], "gateway-cursor-direct");
    assert_eq!(payload["telemetry"]["observed_unix"], 19_000);
    assert_eq!(payload["telemetry"]["hostname"], "source-hostname");
    assert_eq!(payload["telemetry"]["disk_total_bytes"], 1_000);
    assert_eq!(payload["telemetry"]["disk_available_bytes"], 400);
    assert_eq!(payload["telemetry"]["network_rx_bytes"], 400);
    assert_eq!(payload["telemetry"]["network_tx_bytes"], 600);
    assert_eq!(payload["telemetry"]["network_count"], 2);
    assert_eq!(payload["telemetry"]["tunnel_count"], 1);
    assert_eq!(payload["matched_vps"][0]["id"], client_id);
    assert_eq!(
        sqlx::query_as::<_, (i64, i64, i64)>(
            r#"
            SELECT head.accepted_seq, head.projected_seq, cursor.last_sample_seq
            FROM telemetry_projection_heads head
            JOIN telemetry_webhook_cursors cursor USING (client_id)
            WHERE head.client_id=$1
            "#,
        )
        .bind(client_id)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        (1, 1, 1)
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM telemetry_samples WHERE client_id=$1",)
            .bind(client_id)
            .fetch_one(&db.pool)
            .await
            .unwrap(),
        1,
        "cursor consumption must retain the bounded raw journal source"
    );
    assert_eq!(
        sqlx::query_as::<_, (i64, bool)>(
            r#"
            SELECT
                (SELECT count(*) FROM webhook_events WHERE kind='telemetry.rollup'),
                to_regclass('telemetry_webhook_event_receipts') IS NULL
            "#,
        )
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        (0, true)
    );
    assert_eq!(
        process_telemetry_projection_events(&db.pool, WebhookRuleWorkerConfig::default())
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM webhook_rule_deliveries WHERE event_id=$1",
        )
        .bind(&event_id)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        1
    );

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_telemetry_delivery_uses_current_interface_policy_without_hiding_tunnels() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    let client_id = "telemetry-interface-policy-edge";
    insert_webhook_test_client(&db.pool, client_id, "online", false).await;
    insert_webhook_test_rule(
        &db.pool,
        "telemetry interface policy",
        "telemetry.rollup && telemetry.network_count = 1 && telemetry.tunnel_count = 9",
    )
    .await;
    let gateway_session_id = Uuid::new_v4();
    let process_incarnation_id = Uuid::new_v4();
    let metrics = AgentMetrics {
        networks: (0..9)
            .map(|ordinal| vpsman_common::NetworkStat {
                interface: match ordinal {
                    0 => "eth0".to_string(),
                    1 => "docker0".to_string(),
                    8 => "ens8".to_string(),
                    _ => format!("br{ordinal}"),
                },
                rx_bytes: 10 + ordinal,
                tx_bytes: 20 + ordinal,
            })
            .collect(),
        tunnels: (0..9)
            .map(|ordinal| vpsman_common::RuntimeTunnelStat {
                interface: match ordinal {
                    0 => "wg0".to_string(),
                    1 => "tun0".to_string(),
                    8 => "wg8".to_string(),
                    _ => format!("gre{ordinal}"),
                },
                operstate: Some(if ordinal == 1 { "down" } else { "up" }.to_string()),
                rx_bytes: 50 + ordinal,
                tx_bytes: 60 + ordinal,
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    };
    insert_telemetry_projection_sample_with_masks(
        &db.pool,
        client_id,
        1,
        Utc::now(),
        "gateway-interface-policy",
        gateway_session_id,
        process_incarnation_id,
        7,
        30_000,
        &metrics,
        // Projection admitted eth0 + docker0. Current policy narrows docker0
        // and cannot widen ordinal-eight ens8.
        &[0b0000_0011, 0],
        // Projection admitted tun0 + ordinal-eight wg8. Current policy narrows
        // tun0 and cannot widen wg0; neither admitted bit can replace the
        // exact managed-plan identity required to expose tunnel counters.
        &[0b0000_0010, 0b0000_0001],
    )
    .await;
    // Delivery is deliberately later than acceptance: the current rule at
    // materialization owns outward visibility, not the rule that happened to
    // exist when the immutable source sample was accepted.
    sqlx::query(
        r#"
        INSERT INTO vps_rule_values (client_id, key, value_raw, value_json)
        VALUES (
            $1, 'network.interfaces', 'eth*,ens*,wg*',
            '{"mode":"patterns","patterns":["eth*","ens*","wg*"]}'::jsonb
        )
        "#,
    )
    .bind(client_id)
    .execute(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        process_telemetry_projection_events(&db.pool, WebhookRuleWorkerConfig::default())
            .await
            .unwrap(),
        1
    );
    let payload = sqlx::query_scalar::<_, SqlJson<Value>>(
        "SELECT payload FROM webhook_rule_deliveries WHERE event_id LIKE 'telemetry:%'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap()
    .0;
    assert_eq!(payload["telemetry"]["network_count"], 1);
    assert_eq!(payload["telemetry"]["network_rx_bytes"], 10);
    assert_eq!(payload["telemetry"]["network_tx_bytes"], 20);
    assert_eq!(payload["telemetry"]["networks"][0]["interface"], "eth0");
    assert_eq!(payload["telemetry"]["tunnel_count"], 9);
    assert_eq!(payload["telemetry"]["tunnels"][0]["interface"], "wg0");
    assert!(payload["telemetry"]["tunnels"][0].get("rx_bytes").is_none());
    assert_eq!(payload["telemetry"]["tunnels"][1]["interface"], "tun0");
    assert_eq!(payload["telemetry"]["tunnels"][1]["operstate"], "down");
    assert!(payload["telemetry"]["tunnels"][1].get("rx_bytes").is_none());
    assert_eq!(payload["telemetry"]["tunnels"][8]["interface"], "wg8");
    assert!(payload["telemetry"]["tunnels"][8].get("rx_bytes").is_none());
    assert_eq!(payload["telemetry"]["tunnels"][8]["operstate"], "up");
    assert_eq!(
        payload["event"]["predicates"],
        json!([
            "telemetry.network_rate",
            "telemetry.rollup",
            "telemetry.tunnel"
        ])
    );

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_telemetry_cursor_and_delivery_roll_back_together() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    let client_id = "telemetry-cursor-rollback-edge";
    insert_webhook_test_client(&db.pool, client_id, "online", false).await;
    insert_webhook_test_rule(&db.pool, "telemetry cursor rollback", "telemetry.rollup").await;
    insert_telemetry_projection_sample(
        &db.pool,
        client_id,
        1,
        Utc::now() - ChronoDuration::seconds(2),
        "gateway-cursor-rollback",
        Uuid::new_v4(),
        Uuid::new_v4(),
        1,
        18_000,
        &AgentMetrics {
            observed_unix: 18_001,
            hostname: client_id.to_string(),
            ..Default::default()
        },
    )
    .await;
    sqlx::query(
        r#"
        CREATE FUNCTION reject_direct_telemetry_delivery_for_atomicity_test()
        RETURNS trigger
        LANGUAGE plpgsql
        AS $function$
        BEGIN
            RAISE EXCEPTION 'intentional direct telemetry delivery failure';
        END;
        $function$
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        CREATE TRIGGER reject_direct_telemetry_delivery_for_atomicity_test
        BEFORE INSERT ON webhook_rule_deliveries
        FOR EACH ROW
        EXECUTE FUNCTION reject_direct_telemetry_delivery_for_atomicity_test()
        "#,
    )
    .execute(&db.pool)
    .await
    .unwrap();
    let error = process_telemetry_projection_events(&db.pool, WebhookRuleWorkerConfig::default())
        .await
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("intentional direct telemetry delivery failure"));
    assert_eq!(
        sqlx::query_as::<_, (i64, i64, bool)>(
            r#"
            SELECT
                (SELECT last_sample_seq FROM telemetry_webhook_cursors WHERE client_id=$1),
                (SELECT count(*) FROM webhook_rule_deliveries),
                to_regclass('telemetry_webhook_event_receipts') IS NULL
            "#,
        )
        .bind(client_id)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        (0, 0, true)
    );
    sqlx::query(
        "DROP TRIGGER reject_direct_telemetry_delivery_for_atomicity_test ON webhook_rule_deliveries",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query("DROP FUNCTION reject_direct_telemetry_delivery_for_atomicity_test()")
        .execute(&db.pool)
        .await
        .unwrap();
    assert_eq!(
        process_telemetry_projection_events(&db.pool, WebhookRuleWorkerConfig::default())
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_as::<_, (i64, i64)>(
            r#"
            SELECT
                (SELECT last_sample_seq FROM telemetry_webhook_cursors WHERE client_id=$1),
                (SELECT count(*) FROM webhook_rule_deliveries)
            "#,
        )
        .bind(client_id)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        (1, 1)
    );

    sqlx::query(
        "UPDATE telemetry_projection_heads SET accepted_seq=2, projected_seq=2 WHERE client_id=$1",
    )
    .bind(client_id)
    .execute(&db.pool)
    .await
    .unwrap();
    let missing_source =
        process_telemetry_projection_events(&db.pool, WebhookRuleWorkerConfig::default())
            .await
            .unwrap_err();
    assert!(missing_source
        .to_string()
        .contains("telemetry webhook cursor source sample is missing for client"));
    assert_eq!(
        sqlx::query_as::<_, (i64, i64)>(
            r#"
            SELECT
                (SELECT last_sample_seq FROM telemetry_webhook_cursors WHERE client_id=$1),
                (SELECT count(*) FROM webhook_rule_deliveries)
            "#,
        )
        .bind(client_id)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        (1, 1),
        "a missing canonical source must not advance the webhook cursor"
    );

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_no_rule_telemetry_cursor_coalesces_client_backlog_set_wise() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    let client_id = "telemetry-no-rule-coalesced-edge";
    insert_webhook_test_client(&db.pool, client_id, "online", false).await;
    let gateway_session_id = Uuid::new_v4();
    let process_incarnation_id = Uuid::new_v4();
    for sequence in 1..=3 {
        insert_telemetry_projection_sample(
            &db.pool,
            client_id,
            sequence,
            Utc::now() - ChronoDuration::seconds(5 - sequence),
            "gateway-no-rule",
            gateway_session_id,
            process_incarnation_id,
            sequence as u64,
            17_000 + sequence,
            &AgentMetrics {
                observed_unix: 17_100 + sequence as u64,
                hostname: client_id.to_string(),
                ..Default::default()
            },
        )
        .await;
    }
    sqlx::query(
        "UPDATE telemetry_projection_heads SET projected_seq=0, projected_at=NULL WHERE client_id=$1",
    )
    .bind(client_id)
    .execute(&db.pool)
    .await
    .unwrap();

    assert_eq!(
        process_telemetry_projection_events(&db.pool, WebhookRuleWorkerConfig::default())
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_as::<_, (i64, i64, i64, i64)>(
            r#"
            SELECT head.accepted_seq, head.projected_seq, cursor.last_sample_seq,
                   (SELECT count(*) FROM webhook_rule_deliveries)
            FROM telemetry_projection_heads head
            JOIN telemetry_webhook_cursors cursor USING (client_id)
            WHERE head.client_id=$1
            "#,
        )
        .bind(client_id)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        (3, 0, 0, 0)
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM telemetry_samples WHERE client_id=$1",)
            .bind(client_id)
            .fetch_one(&db.pool)
            .await
            .unwrap(),
        3,
        "accepted-but-unprojected telemetry remains owned by projection"
    );
    sqlx::query("UPDATE telemetry_projection_heads SET projected_seq=3 WHERE client_id=$1")
        .bind(client_id)
        .execute(&db.pool)
        .await
        .unwrap();
    assert_eq!(
        process_telemetry_projection_events(&db.pool, WebhookRuleWorkerConfig::default())
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_as::<_, (i64, i64)>(
            r#"
            SELECT cursor.last_sample_seq,
                   (SELECT count(*) FROM telemetry_samples
                    WHERE client_id=$1)
            FROM telemetry_webhook_cursors cursor
            WHERE cursor.client_id=$1
            "#,
        )
        .bind(client_id)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        (3, 3),
        "the later webhook owner advances without pruning bounded raw history"
    );

    sqlx::query(
        "UPDATE telemetry_projection_heads SET accepted_seq=4, projected_seq=4 WHERE client_id=$1",
    )
    .bind(client_id)
    .execute(&db.pool)
    .await
    .unwrap();
    let missing_source =
        process_telemetry_projection_events(&db.pool, WebhookRuleWorkerConfig::default())
            .await
            .unwrap_err();
    assert!(missing_source
        .to_string()
        .contains("telemetry webhook cursor source sample is missing"));
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT last_sample_seq FROM telemetry_webhook_cursors WHERE client_id=$1",
        )
        .bind(client_id)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        3,
        "an autocommit no-rule statement must not partially advance a missing source"
    );

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_no_rule_final_cursor_waits_for_committed_projection_and_prunes_tail() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    let client_id = "telemetry-no-rule-final-tail-edge";
    insert_webhook_test_client(&db.pool, client_id, "online", false).await;
    insert_telemetry_projection_sample(
        &db.pool,
        client_id,
        1,
        Utc::now(),
        "gateway-no-rule-final-tail",
        Uuid::new_v4(),
        Uuid::new_v4(),
        1,
        18_000,
        &AgentMetrics {
            observed_unix: 18_001,
            hostname: client_id.to_string(),
            ..Default::default()
        },
    )
    .await;
    sqlx::query(
        "UPDATE telemetry_projection_heads SET projected_seq=0, projected_at=NULL WHERE client_id=$1",
    )
    .bind(client_id)
    .execute(&db.pool)
    .await
    .unwrap();

    // Hold the final projection advance uncommitted. The no-rule owner must
    // observe the prior committed projection frontier, so it cannot acknowledge
    // accepted telemetry early and recreate the former two-trigger tail race.
    let mut projection = db.pool.begin().await.unwrap();
    sqlx::query("UPDATE telemetry_projection_heads SET projected_seq=1 WHERE client_id=$1")
        .bind(client_id)
        .execute(&mut *projection)
        .await
        .unwrap();
    assert!(
        try_advance_telemetry_webhook_cursor_without_enabled_rules(&db.pool, 100)
            .await
            .unwrap()
    );
    assert_eq!(
        sqlx::query_as::<_, (i64, i64)>(
            r#"
            SELECT cursor.last_sample_seq,
                   (SELECT count(*) FROM telemetry_samples
                    WHERE client_id=$1)
            FROM telemetry_webhook_cursors cursor
            WHERE cursor.client_id=$1
            "#,
        )
        .bind(client_id)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        (0, 1)
    );

    projection.commit().await.unwrap();
    assert!(
        try_advance_telemetry_webhook_cursor_without_enabled_rules(&db.pool, 100)
            .await
            .unwrap()
    );
    assert_eq!(
        sqlx::query_as::<_, (i64, i64, i64, i64)>(
            r#"
            SELECT head.accepted_seq, head.projected_seq,
                   cursor.last_sample_seq,
                   (SELECT count(*) FROM telemetry_samples
                    WHERE client_id=$1)
            FROM telemetry_projection_heads head
            JOIN telemetry_webhook_cursors cursor USING (client_id)
            WHERE head.client_id=$1
            "#,
        )
        .bind(client_id)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        (1, 1, 1, 1),
        "the committed projection and webhook cursor retain bounded raw history"
    );

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_concurrent_rule_enable_preserves_the_commit_boundary() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    let client_id = "telemetry-rule-enable-boundary-edge";
    insert_webhook_test_client(&db.pool, client_id, "online", false).await;
    let gateway_session_id = Uuid::new_v4();
    let process_incarnation_id = Uuid::new_v4();
    insert_telemetry_projection_sample(
        &db.pool,
        client_id,
        1,
        Utc::now() - ChronoDuration::seconds(1),
        "gateway-rule-enable-boundary",
        gateway_session_id,
        process_incarnation_id,
        1,
        21_000,
        &AgentMetrics {
            observed_unix: 21_001,
            hostname: client_id.to_string(),
            ..Default::default()
        },
    )
    .await;

    let rule_id = Uuid::new_v4();
    let mut enabling = db.pool.begin().await.unwrap();
    sqlx::query(
        r#"
        INSERT INTO webhook_rules (
            id, name, enabled, expression, target, body_template, cooldown_secs
        )
        VALUES (
            $1, 'telemetry rule enable boundary', TRUE, 'telemetry.rollup',
            'https://hooks.example.invalid/vpsman', '', 0
        )
        "#,
    )
    .bind(rule_id)
    .execute(&mut *enabling)
    .await
    .unwrap();

    assert_eq!(
        tokio::time::timeout(
            Duration::from_secs(5),
            process_telemetry_projection_events(&db.pool, WebhookRuleWorkerConfig::default()),
        )
        .await
        .expect("an uncommitted rule enable must not block the no-rule cursor")
        .unwrap(),
        0
    );
    enabling.commit().await.unwrap();
    assert_eq!(
        sqlx::query_as::<_, (i64, i64)>(
            r#"
            SELECT cursor.last_sample_seq, count(delivery.id)::bigint
            FROM telemetry_webhook_cursors cursor
            LEFT JOIN webhook_rule_deliveries delivery ON delivery.rule_id = $2
            WHERE cursor.client_id = $1
            GROUP BY cursor.last_sample_seq
            "#,
        )
        .bind(client_id)
        .bind(rule_id)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        (1, 0),
        "telemetry committed before rule activation retains the no-rule outcome"
    );

    insert_telemetry_projection_sample(
        &db.pool,
        client_id,
        2,
        Utc::now(),
        "gateway-rule-enable-boundary",
        gateway_session_id,
        process_incarnation_id,
        2,
        21_001,
        &AgentMetrics {
            observed_unix: 21_002,
            hostname: client_id.to_string(),
            ..Default::default()
        },
    )
    .await;
    assert_eq!(
        process_telemetry_projection_events(&db.pool, WebhookRuleWorkerConfig::default())
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_as::<_, (i64, i64)>(
            r#"
            SELECT cursor.last_sample_seq, count(delivery.id)::bigint
            FROM telemetry_webhook_cursors cursor
            LEFT JOIN webhook_rule_deliveries delivery ON delivery.rule_id = $2
            WHERE cursor.client_id = $1
            GROUP BY cursor.last_sample_seq
            "#,
        )
        .bind(client_id)
        .bind(rule_id)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        (2, 1),
        "telemetry committed after rule activation is materialized exactly once"
    );

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_validated_constraint_rejects_full_telemetry_outbox() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    assert!(sqlx::query_scalar::<_, bool>(
        r#"
        SELECT convalidated
        FROM pg_constraint
        WHERE conrelid='webhook_events'::regclass
          AND conname='webhook_events_no_full_telemetry_outbox'
        "#,
    )
    .fetch_one(&db.pool)
    .await
    .unwrap());
    let error = insert_webhook_event(
        &db.pool,
        "telemetry.rollup",
        "telemetry:prohibited-full-outbox",
        &["telemetry.rollup"],
        &[],
        json!({"telemetry":{"client_id":"prohibited"}}),
    )
    .await
    .unwrap_err();
    assert!(format!("{error:#}").contains("webhook_events_no_full_telemetry_outbox"));
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM webhook_events WHERE kind='telemetry.rollup'",
        )
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        0
    );

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_event_retention_skips_owned_rows_and_never_blocks_producers() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    let expired_event_id = Uuid::new_v4();
    let expired_occurred_at = Utc::now() - ChronoDuration::days(3);
    sqlx::query(
        r#"
        INSERT INTO webhook_events (
            id, kind, event_id, payload, occurred_at, processed_at
        )
        VALUES ($1, 'retention.concurrent_test', $2, '{}'::jsonb, $3, now())
        "#,
    )
    .bind(expired_event_id)
    .bind(format!("retention-expired:{expired_event_id}"))
    .bind(expired_occurred_at)
    .execute(&db.pool)
    .await
    .unwrap();

    let duplicate_id = sqlx::query(
        r#"
        INSERT INTO webhook_events (
            id, kind, event_id, payload, occurred_at, processed_at
        )
        VALUES ($1, 'retention.concurrent_test', $2, '{}'::jsonb, $3, now())
        "#,
    )
    .bind(expired_event_id)
    .bind(format!("retention-duplicate-id:{expired_event_id}"))
    .bind(expired_occurred_at + ChronoDuration::hours(1))
    .execute(&db.pool)
    .await
    .unwrap_err();
    assert_eq!(
        duplicate_id
            .as_database_error()
            .and_then(|error| error.constraint()),
        Some("webhook_events_pkey")
    );

    let primary_key: String = sqlx::query_scalar(
        r#"
        SELECT pg_get_constraintdef(oid)
        FROM pg_constraint
        WHERE conrelid='webhook_events'::regclass
          AND conname='webhook_events_pkey'
        "#,
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(primary_key, "PRIMARY KEY (id)");

    let mut planner = db.pool.begin().await.unwrap();
    sqlx::query("SET LOCAL enable_seqscan=off")
        .execute(&mut *planner)
        .await
        .unwrap();
    let processed_plan = sqlx::query_scalar::<_, String>(
        r#"
        EXPLAIN SELECT occurred_at, id
        FROM webhook_events
        WHERE processed_at IS NOT NULL
          AND occurred_at <= now() - interval '1 day'
        ORDER BY occurred_at, id
        LIMIT 1000
        "#,
    )
    .fetch_all(&mut *planner)
    .await
    .unwrap()
    .join("\n");
    assert!(processed_plan.contains("webhook_events_processed_retention_idx"));
    let unprocessed_plan = sqlx::query_scalar::<_, String>(
        r#"
        EXPLAIN SELECT occurred_at, id
        FROM webhook_events
        WHERE processed_at IS NULL
        ORDER BY occurred_at, id
        LIMIT 100
        "#,
    )
    .fetch_all(&mut *planner)
    .await
    .unwrap()
    .join("\n");
    assert!(unprocessed_plan.contains("webhook_events_unprocessed_idx"));
    planner.rollback().await.unwrap();

    let mut event_owner = db.pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM webhook_events WHERE id=$1 FOR UPDATE")
        .bind(expired_event_id)
        .fetch_one(&mut *event_owner)
        .await
        .unwrap();

    let config = WebhookRuleWorkerConfig::new(25, 100, 1, 1_000, 5).unwrap();
    let live_event_id = Uuid::new_v4();
    let live_event_key = format!("retention-live:{live_event_id}");
    let prune = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        prune_webhook_events(&db.pool, config),
    );
    let insert = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        insert_webhook_event(
            &db.pool,
            "retention.concurrent_test",
            &live_event_key,
            &["retention.concurrent_test"],
            &[],
            json!({"event": {"kind": "retention.concurrent_test"}}),
        ),
    );
    let (pruned_while_owned, inserted_while_retaining) = tokio::join!(prune, insert);
    assert_eq!(
        pruned_while_owned
            .expect("retention waited on an independently owned event row")
            .unwrap(),
        0
    );
    assert!(inserted_while_retaining
        .expect("retention blocked a webhook event producer")
        .unwrap());

    event_owner.rollback().await.unwrap();
    assert_eq!(prune_webhook_events(&db.pool, config).await.unwrap(), 1);

    let (expired_exists, live_exists): (bool, bool) = sqlx::query_as(
        r#"
        SELECT
            EXISTS (SELECT 1 FROM webhook_events WHERE id=$1),
            EXISTS (SELECT 1 FROM webhook_events WHERE event_id=$2)
        "#,
    )
    .bind(expired_event_id)
    .bind(&live_event_key)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert!(!expired_exists);
    assert!(live_exists, "unprocessed event must remain durable");

    let relation_kind: String = sqlx::query_scalar(
        "SELECT relkind::text FROM pg_class WHERE oid='webhook_events'::regclass",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        relation_kind, "r",
        "webhook event outbox must be an ordinary table"
    );
    assert!(!sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM pg_tables
            WHERE schemaname=current_schema()
              AND tablename ~ '^webhook_events_[0-9]{8}$'
        )
        "#,
    )
    .fetch_one(&db.pool)
    .await
    .unwrap());

    sqlx::query("DELETE FROM webhook_events WHERE event_id=$1")
        .bind(&live_event_key)
        .execute(&db.pool)
        .await
        .unwrap();
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_generic_alert_resolution_materializes_once_for_an_explicit_retained_subject() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    insert_webhook_test_client(&db.pool, "retained-generic-edge", "deleted", true).await;
    let stable_rule = insert_webhook_test_rule(
        &db.pool,
        "retained-generic-resolution",
        "alert.resolved && alert.category:job && alert.record_kind = event",
    )
    .await;
    let mutable_vps_rule = insert_webhook_test_rule(
        &db.pool,
        "retained-generic-resolution-live-state",
        "alert.resolved && status = deleted",
    )
    .await;
    let episode_id = Uuid::new_v4();
    let event_id = format!("fleet-alert:{episode_id}:resolved");
    let subject_client_ids = vec!["retained-generic-edge".to_string()];
    let payload = json!({
        "event": {
            "kind": "alert.resolved",
            "id": &event_id,
            "occurred_at": "2026-08-18T12:00:00Z",
        },
        "alert": {
            "id": "job:failed:job-1",
            "episode_id": episode_id,
            "record_kind": "event",
            "producer_kind": "job",
            "trigger_generation": 1,
            "lifecycle_state": "resolved",
            "severity": "critical",
            "category": "job",
            "client_id": "retained-generic-edge",
            "resolved_at": "2026-08-18T12:00:00Z",
            "resolution_reason": "operator_resolved",
        },
    });
    assert!(insert_webhook_event(
        &db.pool,
        "alert.resolved",
        &event_id,
        &[
            "alert.resolved",
            "alert.category:job",
            "alert.severity:critical",
        ],
        &subject_client_ids,
        payload.clone(),
    )
    .await
    .unwrap());
    assert!(!insert_webhook_event(
        &db.pool,
        "alert.resolved",
        &event_id,
        &[
            "alert.resolved",
            "alert.category:job",
            "alert.severity:critical",
        ],
        &subject_client_ids,
        payload,
    )
    .await
    .unwrap());

    assert_eq!(
        process_webhook_events(&db.pool, WebhookRuleWorkerConfig::default())
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        process_webhook_events(&db.pool, WebhookRuleWorkerConfig::default())
            .await
            .unwrap(),
        0
    );

    let deliveries = sqlx::query_as::<_, (Uuid, String, String, SqlJson<Value>, SqlJson<Value>)>(
        r#"
        SELECT rule_id, event_kind, event_id, payload, matched_vps
        FROM webhook_rule_deliveries
        ORDER BY rule_id
        "#,
    )
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert_eq!(deliveries.len(), 1);
    assert_eq!(deliveries[0].0, stable_rule);
    assert_ne!(deliveries[0].0, mutable_vps_rule);
    assert_eq!(deliveries[0].1, "alert.resolved");
    assert_eq!(deliveries[0].2, event_id);
    assert_eq!(
        deliveries[0].3 .0["alert"]["episode_id"],
        episode_id.to_string()
    );
    assert_eq!(
        deliveries[0].3 .0["alert"]["resolution_reason"],
        "operator_resolved"
    );
    assert_eq!(deliveries[0].4 .0[0]["id"], "retained-generic-edge");
    assert_eq!(deliveries[0].4 .0[0]["status"], "deleted");

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_subjectless_interval_excludes_retained_subjects_loaded_for_other_events() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    insert_webhook_test_client(&db.pool, "visible-edge", "online", false).await;
    insert_webhook_test_client(&db.pool, "retained-edge", "deleted", true).await;
    let interval_rule =
        insert_webhook_test_rule(&db.pool, "visible-interval", "interval.30sec").await;
    assert!(insert_webhook_event(
        &db.pool,
        "agent.test",
        "retained-subject-batch-loader",
        &["agent.test"],
        &["retained-edge".to_string()],
        json!({"event": {"kind": "agent.test"}}),
    )
    .await
    .unwrap());
    assert!(insert_webhook_event(
        &db.pool,
        "interval.30sec",
        "interval.30sec:retained-regression",
        &["interval.30sec"],
        &[],
        json!({"event": {"kind": "interval.30sec"}}),
    )
    .await
    .unwrap());

    assert_eq!(
        process_webhook_events(&db.pool, WebhookRuleWorkerConfig::default())
            .await
            .unwrap(),
        1
    );
    let (rule_id, matched_vps) = sqlx::query_as::<_, (Uuid, SqlJson<Value>)>(
        "SELECT rule_id, matched_vps FROM webhook_rule_deliveries",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(rule_id, interval_rule);
    assert_eq!(matched_vps.0.as_array().map(Vec::len), Some(1));
    assert_eq!(matched_vps.0[0]["id"], "visible-edge");

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_subjectless_generic_alert_edges_do_not_borrow_fleet_subjects() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    let event_only_rule = insert_webhook_test_rule(
        &db.pool,
        "global-alert-event-only",
        "(alert.triggered || alert.resolved) && alert.category:job",
    )
    .await;
    let vps_dependent_rule = insert_webhook_test_rule(
        &db.pool,
        "global-alert-vps-dependent",
        "(alert.triggered || alert.resolved) && status = online",
    )
    .await;
    let edge_payload = |kind: &str, event_id: &str, state: &str| {
        json!({
            "event": {"kind": kind, "id": event_id},
            "alert": {
                "id": "job:failed:global-job",
                "episode_id": Uuid::nil(),
                "record_kind": "event",
                "producer_kind": "job",
                "lifecycle_state": state,
                "severity": "critical",
                "category": "job",
                "target_kind": "job",
                "target_id": "global-job",
                "client_id": null,
            },
        })
    };

    assert!(insert_webhook_event(
        &db.pool,
        "alert.triggered",
        "fleet-alert:global-empty:triggered",
        &[
            "alert.triggered",
            "alert.category:job",
            "alert.severity:critical",
        ],
        &[],
        edge_payload(
            "alert.triggered",
            "fleet-alert:global-empty:triggered",
            "triggered",
        ),
    )
    .await
    .unwrap());
    assert_eq!(
        process_webhook_events(&db.pool, WebhookRuleWorkerConfig::default())
            .await
            .unwrap(),
        1
    );

    insert_webhook_test_client(&db.pool, "unrelated-visible-edge", "online", false).await;
    assert!(insert_webhook_event(
        &db.pool,
        "alert.resolved",
        "fleet-alert:global-visible:resolved",
        &[
            "alert.resolved",
            "alert.category:job",
            "alert.severity:critical",
        ],
        &[],
        edge_payload(
            "alert.resolved",
            "fleet-alert:global-visible:resolved",
            "resolved",
        ),
    )
    .await
    .unwrap());
    assert!(!insert_webhook_event(
        &db.pool,
        "alert.resolved",
        "fleet-alert:global-visible:resolved",
        &[
            "alert.resolved",
            "alert.category:job",
            "alert.severity:critical",
        ],
        &[],
        edge_payload(
            "alert.resolved",
            "fleet-alert:global-visible:resolved",
            "resolved",
        ),
    )
    .await
    .unwrap());
    assert_eq!(
        process_webhook_events(&db.pool, WebhookRuleWorkerConfig::default())
            .await
            .unwrap(),
        1
    );

    let deliveries = sqlx::query_as::<_, (Uuid, String, SqlJson<Value>)>(
        r#"
        SELECT rule_id, event_kind, matched_vps
        FROM webhook_rule_deliveries
        ORDER BY event_kind DESC
        "#,
    )
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert_eq!(deliveries.len(), 2);
    assert!(deliveries
        .iter()
        .all(|delivery| delivery.0 == event_only_rule));
    assert!(deliveries
        .iter()
        .all(|delivery| delivery.0 != vps_dependent_rule));
    assert_eq!(deliveries[0].1, "alert.triggered");
    assert_eq!(deliveries[1].1, "alert.resolved");
    assert!(deliveries
        .iter()
        .all(|delivery| delivery.2 .0.as_array().is_some_and(Vec::is_empty)));

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_suspended_client_alert_trigger_materializes_neutral_cancellation() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    let client_id = format!("suspended-alert-{}", Uuid::new_v4().simple());
    insert_webhook_test_client(&db.pool, &client_id, "offline", false).await;
    sqlx::query(
        r#"
        UPDATE clients
        SET status='suspended', suspended_at=now(),
            suspended_reason='test', suspended_from_status='offline'
        WHERE id=$1
        "#,
    )
    .bind(&client_id)
    .execute(&db.pool)
    .await
    .unwrap();
    insert_webhook_test_rule(&db.pool, "suspended-alert-trigger", "alert.triggered").await;
    let event_id = format!("fleet-alert:{client_id}:triggered");
    assert!(insert_webhook_event(
        &db.pool,
        "alert.triggered",
        &event_id,
        &["alert.triggered", "alert.category:agent_status"],
        std::slice::from_ref(&client_id),
        json!({
            "event": {"kind": "alert.triggered", "id": &event_id},
            "alert": {
                "id": format!("agent_status:agent:{client_id}"),
                "record_kind": "condition",
                "lifecycle_state": "triggered",
                "category": "agent_status",
                "client_id": &client_id,
            },
        }),
    )
    .await
    .unwrap());

    assert_eq!(
        process_webhook_events(&db.pool, WebhookRuleWorkerConfig::default())
            .await
            .unwrap(),
        1
    );
    let delivery = sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT status, error FROM webhook_rule_deliveries",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        delivery,
        (
            WEBHOOK_RULE_DELIVERY_STATUS_CANCELED_DISABLED.to_string(),
            Some("client_suspended".to_string())
        )
    );
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_delivery_consumer_terminalizes_suspended_durable_work() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    let client_id = format!("delivery-consumer-{}", Uuid::new_v4().simple());
    insert_webhook_test_client(&db.pool, &client_id, "offline", false).await;
    let rule_id =
        insert_webhook_test_rule(&db.pool, "delivery-consumer-alert", "alert.triggered").await;
    let delivery_id = Uuid::new_v4();
    insert_claimed_client_alert_webhook_delivery(
        &db.pool,
        rule_id,
        delivery_id,
        Uuid::new_v4(),
        &client_id,
    )
    .await;
    sqlx::query(
        r#"
        UPDATE webhook_rule_deliveries
        SET status='queued', delivery_lease_id=NULL, delivery_lease_until=NULL
        WHERE id=$1
        "#,
    )
    .bind(delivery_id)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        UPDATE clients
        SET status='suspended', suspended_at=now(),
            suspended_reason='test', suspended_from_status='offline'
        WHERE id=$1
        "#,
    )
    .bind(&client_id)
    .execute(&db.pool)
    .await
    .unwrap();

    let run = process_due_webhook_deliveries(&db.pool, WebhookRuleWorkerConfig::default())
        .await
        .unwrap();
    assert_eq!((run.processed, run.delivered, run.failed), (1, 0, 1));
    assert_eq!(
        sqlx::query_as::<_, (String, Option<String>, i32)>(
            "SELECT status, error, attempt_count FROM webhook_rule_deliveries WHERE id=$1",
        )
        .bind(delivery_id)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        (
            WEBHOOK_RULE_DELIVERY_STATUS_CANCELED_DISABLED.to_string(),
            Some("client_suspended".to_string()),
            0,
        )
    );
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_alert_materialization_does_not_own_client_lifecycle_rows() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    let client_id = format!("materialize-unlocked-{}", Uuid::new_v4().simple());
    insert_webhook_test_client(&db.pool, &client_id, "offline", false).await;
    insert_webhook_test_rule(&db.pool, "unlocked-alert-trigger", "alert.triggered").await;
    let event_id = format!("fleet-alert:{client_id}:triggered");
    assert!(insert_webhook_event(
        &db.pool,
        "alert.triggered",
        &event_id,
        &["alert.triggered", "alert.category:agent_status"],
        std::slice::from_ref(&client_id),
        json!({
            "event": {"kind": "alert.triggered", "id": &event_id},
            "alert": {
                "id": format!("agent_status:agent:{client_id}"),
                "record_kind": "condition",
                "lifecycle_state": "triggered",
                "category": "agent_status",
                "client_id": &client_id,
            },
        }),
    )
    .await
    .unwrap());

    let mut lifecycle_writer = db.pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM clients WHERE id=$1 FOR NO KEY UPDATE")
        .bind(&client_id)
        .fetch_one(&mut *lifecycle_writer)
        .await
        .unwrap();
    assert_eq!(
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            process_webhook_events(&db.pool, WebhookRuleWorkerConfig::default()),
        )
        .await
        .expect("webhook materialization blocked on a client lifecycle producer")
        .unwrap(),
        1
    );
    lifecycle_writer.rollback().await.unwrap();

    let (status, error): (String, Option<String>) =
        sqlx::query_as("SELECT status, error FROM webhook_rule_deliveries WHERE event_id=$1")
            .bind(event_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(status, "queued");
    assert!(error.is_none());
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_unsuspend_never_resurrects_a_pre_suspension_alert_trigger() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    let client_id = format!("pending-alert-{}", Uuid::new_v4().simple());
    insert_webhook_test_client(&db.pool, &client_id, "offline", false).await;
    insert_webhook_test_rule(&db.pool, "pending-alert-trigger", "alert.triggered").await;
    let (episode_id, event_id, event_seq, payload) =
        insert_pending_client_alert_lifecycle_source(&db.pool, &client_id).await;

    // Model the durable part of suspend_agent followed by manual unsuspend.
    // The client is eligible again, but the old immutable episode generation
    // remains marked as suppressed and cannot be revived.
    sqlx::query(
        r#"
        UPDATE clients
        SET status='suspended', suspended_at=now(),
            suspended_reason='test', suspended_from_status='offline'
        WHERE id=$1
        "#,
    )
    .bind(&client_id)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        UPDATE alert_episodes
        SET lifecycle_state='resolved', resolved_at=now(),
            resolution_reason='source_scope_exited',
            evidence=evidence || jsonb_build_object(
                '_vpsman_client_suspension',
                jsonb_build_object('client_id',$2::text,'suppressed_at',now())
            ),
            updated_at=now()
        WHERE id=$1
        "#,
    )
    .bind(episode_id)
    .bind(&client_id)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO audit_logs (id, action, target, metadata)
        VALUES (
            $1,'agent.suspended',$2,
            '{"result":"succeeded","origin_kind":"operator_request","component":"inventory-controller"}'::jsonb
        )
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(format!("client:{client_id}"))
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        UPDATE clients
        SET status='offline', suspended_at=NULL, suspended_by=NULL,
            suspended_reason=NULL, suspended_from_status=NULL
        WHERE id=$1
        "#,
    )
    .bind(&client_id)
    .execute(&db.pool)
    .await
    .unwrap();

    // Projection happens only after unsuspend. The immutable episode marker,
    // rather than the client's current status, rejects this old generation.
    let webhook_event_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO webhook_events (
            id, kind, event_id, event_predicates, subject_client_ids,
            payload, occurred_at, alert_lifecycle_event_seq
        ) VALUES (
            $1,'alert.triggered',$2,
            ARRAY['alert.triggered','alert.category:agent_status','alert.severity:warning'],
            ARRAY[$3]::text[],$4,now(),$5
        )
        "#,
    )
    .bind(webhook_event_id)
    .bind(&event_id)
    .bind(&client_id)
    .bind(SqlJson(payload))
    .bind(event_seq)
    .execute(&db.pool)
    .await
    .unwrap();

    assert_eq!(
        process_webhook_events(&db.pool, WebhookRuleWorkerConfig::default())
            .await
            .unwrap(),
        1
    );
    let delivery = sqlx::query_as::<_, (String, Option<String>, i32, Option<DateTime<Utc>>)>(
        r#"
        SELECT status, error, attempt_count, delivered_at
        FROM webhook_rule_deliveries WHERE event_id=$1
        "#,
    )
    .bind(&event_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(delivery.0, WEBHOOK_RULE_DELIVERY_STATUS_CANCELED_DISABLED);
    assert_eq!(delivery.1.as_deref(), Some("client_suspended"));
    assert_eq!(delivery.2, 0);
    assert!(delivery.3.is_none());
    assert!(sqlx::query_scalar::<_, bool>(
        "SELECT processed_at IS NOT NULL FROM webhook_events WHERE id=$1",
    )
    .bind(webhook_event_id)
    .fetch_one(&db.pool)
    .await
    .unwrap());
    let retained = sqlx::query_as::<_, (String, String)>(
        r#"
        SELECT lifecycle_state,
               evidence#>>'{_vpsman_client_suspension,client_id}'
        FROM alert_episodes WHERE id=$1
        "#,
    )
    .bind(episode_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(retained, ("resolved".to_string(), client_id.clone()));
    assert!(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM audit_logs WHERE target=$1 AND action='agent.suspended')",
    )
    .bind(format!("client:{client_id}"))
    .fetch_one(&db.pool)
    .await
    .unwrap());
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_client_alert_webhook_revision_rejects_completion_after_suspension() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    let client_id = format!("webhook-alert-fence-{}", Uuid::new_v4().simple());
    insert_webhook_test_client(&db.pool, &client_id, "offline", false).await;
    let rule_id =
        insert_webhook_test_rule(&db.pool, "webhook-alert-fence", "alert.triggered").await;
    let delivery_id = Uuid::new_v4();
    let lease_id = Uuid::new_v4();
    insert_claimed_client_alert_webhook_delivery(
        &db.pool,
        rule_id,
        delivery_id,
        lease_id,
        &client_id,
    )
    .await;
    let delivery = DeliveryRow {
        id: delivery_id,
        rule_id,
        actor_id: None,
        rule_name: "webhook-alert-fence".to_string(),
        event_kind: "alert.triggered".to_string(),
        event_id: format!("fleet-alert:{client_id}:triggered"),
        target: "https://hooks.example.invalid/vpsman".to_string(),
        signing_secret: None,
        payload: json!({}),
        attempt_count: 0,
    };
    let send_eligibility = begin_client_alert_webhook_send(&db.pool, delivery_id, lease_id)
        .await
        .unwrap();
    assert_eq!(
        send_eligibility.eligibility,
        ClientAlertWebhookSendEligibility::Deliverable
    );
    let revision = send_eligibility
        .revision
        .expect("deliverable webhook must arm a durable eligibility revision");

    let mut suspension = db.pool.begin().await.unwrap();
    sqlx::query(
        r#"
        UPDATE clients
        SET status='suspended', suspended_at=now(),
            suspended_reason='test', suspended_from_status='offline'
        WHERE id=$1
        "#,
    )
    .bind(&client_id)
    .execute(&mut *suspension)
    .await
    .unwrap();
    sqlx::query(
        r#"
        UPDATE webhook_rule_deliveries delivery
        SET status='canceled_disabled', error='client_suspended',
            delivery_lease_id=NULL, delivery_lease_until=NULL
        WHERE delivery.event_kind='alert.triggered'
          AND delivery.status IN ('queued','failed','in_progress')
          AND EXISTS (
                SELECT 1 FROM jsonb_array_elements(delivery.matched_vps) matched
                WHERE matched->>'id'=$1
          )
        "#,
    )
    .bind(&client_id)
    .execute(&mut *suspension)
    .await
    .unwrap();
    suspension.commit().await.unwrap();

    let recorded = complete_webhook_rule_delivery_on_pool(
        &db.pool,
        &delivery,
        lease_id,
        Some(revision),
        WEBHOOK_RULE_DELIVERY_STATUS_DELIVERED,
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(recorded, None);
    let status: String =
        sqlx::query_scalar("SELECT status FROM webhook_rule_deliveries WHERE id=$1")
            .bind(delivery_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(status, WEBHOOK_RULE_DELIVERY_STATUS_CANCELED_DISABLED);

    let blocked_delivery_id = Uuid::new_v4();
    let blocked_lease_id = Uuid::new_v4();
    insert_claimed_client_alert_webhook_delivery(
        &db.pool,
        rule_id,
        blocked_delivery_id,
        blocked_lease_id,
        &client_id,
    )
    .await;
    let blocked_guard =
        begin_client_alert_webhook_send(&db.pool, blocked_delivery_id, blocked_lease_id)
            .await
            .unwrap();
    assert_eq!(
        blocked_guard.eligibility,
        ClientAlertWebhookSendEligibility::ClientSuspended
    );

    for mutation in ["matched_vps || matched_vps", "matched_vps || '[{}]'::jsonb"] {
        let invalid_delivery_id = Uuid::new_v4();
        let invalid_lease_id = Uuid::new_v4();
        insert_claimed_client_alert_webhook_delivery(
            &db.pool,
            rule_id,
            invalid_delivery_id,
            invalid_lease_id,
            &client_id,
        )
        .await;
        sqlx::query(&format!(
            "UPDATE webhook_rule_deliveries SET matched_vps={mutation} WHERE id=$1"
        ))
        .bind(invalid_delivery_id)
        .execute(&db.pool)
        .await
        .unwrap();
        let invalid =
            begin_client_alert_webhook_send(&db.pool, invalid_delivery_id, invalid_lease_id)
                .await
                .unwrap();
        assert_eq!(
            invalid.eligibility,
            ClientAlertWebhookSendEligibility::InvalidClientScope,
            "duplicate and malformed client snapshots must fail closed"
        );
    }

    sqlx::query("UPDATE webhook_rules SET enabled=FALSE WHERE id=$1")
        .bind(rule_id)
        .execute(&db.pool)
        .await
        .unwrap();
    let disabled_delivery_id = Uuid::new_v4();
    let disabled_lease_id = Uuid::new_v4();
    insert_claimed_client_alert_webhook_delivery(
        &db.pool,
        rule_id,
        disabled_delivery_id,
        disabled_lease_id,
        &client_id,
    )
    .await;
    let disabled_guard =
        begin_client_alert_webhook_send(&db.pool, disabled_delivery_id, disabled_lease_id)
            .await
            .unwrap();
    assert_eq!(
        disabled_guard.eligibility,
        ClientAlertWebhookSendEligibility::RuleDisabled,
        "a concurrent rule disable retains its canonical cancellation reason"
    );
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_permanent_failures_remain_delivery_and_audit_evidence_only() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    insert_webhook_test_rule(&db.pool, "invalid-no-recursive-alert", "(alert.triggered").await;
    assert!(insert_webhook_event(
        &db.pool,
        "alert.triggered",
        "fleet-alert:no-recursion:triggered",
        &["alert.triggered", "alert.category:job"],
        &[],
        json!({
            "event": {
                "kind": "alert.triggered",
                "id": "fleet-alert:no-recursion:triggered",
            },
            "alert": {
                "id": "job:failed:no-recursion",
                "record_kind": "event",
                "lifecycle_state": "triggered",
                "category": "job",
            },
        }),
    )
    .await
    .unwrap());

    assert_eq!(
        process_webhook_events(&db.pool, WebhookRuleWorkerConfig::default())
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        process_webhook_events(&db.pool, WebhookRuleWorkerConfig::default())
            .await
            .unwrap(),
        0
    );
    let permanent_deliveries: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM webhook_rule_deliveries WHERE status = 'permanently_failed'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let permanent_failure_audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_logs WHERE action = 'webhook.rule_delivery_permanently_failed'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let fabricated_states: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM fleet_alert_states WHERE alert_id LIKE 'webhook_delivery:%'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let recursive_events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM webhook_events WHERE event_id <> 'fleet-alert:no-recursion:triggered'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(permanent_deliveries, 1);
    assert_eq!(permanent_failure_audits, 1);
    assert_eq!(fabricated_states, 0);
    assert_eq!(recursive_events, 0);

    db.cleanup().await;
}

#[tokio::test]
async fn postgres_delivery_retention_never_changes_operator_triage() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    let rule_id =
        insert_webhook_test_rule(&db.pool, "retention-does-not-triage", "alert.triggered").await;
    let delivery_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO webhook_rule_deliveries (
            id, rule_id, rule_name, event_kind, event_id, status, target,
            dedupe_key, payload, matched_vps, message, cooldown_until_unix,
            created_at
        )
        VALUES (
            $1, $2, 'retention-does-not-triage', 'alert.triggered',
            'fleet-alert:retention:triggered', 'permanently_failed',
            'https://hooks.example.invalid/vpsman', 'retention-no-triage',
            '{}'::jsonb, '[]'::jsonb, 'failed', 0,
            now() - interval '2 days'
        )
        "#,
    )
    .bind(delivery_id)
    .bind(rule_id)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO fleet_alert_states (alert_id, state, reason)
        VALUES ($1, 'open', 'operator-owned legacy triage')
        "#,
    )
    .bind(format!("webhook_delivery:{delivery_id}"))
    .execute(&db.pool)
    .await
    .unwrap();

    let config = WebhookRuleWorkerConfig::new(25, 100, 1, 1_000, 5).unwrap();
    assert_eq!(prune_deliveries(&db.pool, config).await.unwrap(), 1);
    let triage = sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT state, reason FROM fleet_alert_states WHERE alert_id = $1",
    )
    .bind(format!("webhook_delivery:{delivery_id}"))
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(triage.0, "open");
    assert_eq!(triage.1.as_deref(), Some("operator-owned legacy triage"));

    db.cleanup().await;
}

#[test]
fn delivery_error_is_bounded() {
    let error = "x".repeat(MAX_ERROR_BYTES + 100);
    assert_eq!(truncate_error(&error).len(), MAX_ERROR_BYTES);
}

#[test]
fn delivery_error_keeps_nested_transport_cause() {
    let error = anyhow::anyhow!("connection refused").context("webhook request failed");
    assert_eq!(
        format_delivery_error(&error),
        "webhook request failed: connection refused"
    );
}

#[test]
fn automatic_delivery_cooldown_blocks_new_events_but_not_boundary_event() {
    assert!(delivery_candidate_is_suppressed(
        false,
        1_300,
        1_299,
        "job.created"
    ));
    assert!(!delivery_candidate_is_suppressed(
        false,
        1_300,
        1_300,
        "job.created"
    ));
    assert!(delivery_candidate_is_suppressed(
        true,
        0,
        1_300,
        "job.created"
    ));
}

#[test]
fn alert_lifecycle_edges_bypass_rule_cooldown_but_keep_exact_dedupe() {
    for event_kind in ["alert.triggered", "alert.resolved"] {
        assert!(!delivery_candidate_is_suppressed(
            false, 1_300, 1_100, event_kind
        ));
        assert!(delivery_candidate_is_suppressed(
            true, 1_300, 1_100, event_kind
        ));
    }
    assert!(delivery_candidate_is_suppressed(
        false,
        1_300,
        1_100,
        "job.created"
    ));
}

#[test]
fn enabled_rule_pagination_advances_and_interval_checks_include_later_pages() {
    let rule = |id, expression: &str| RuleRow {
        id: Uuid::from_u128(id),
        actor_id: None,
        name: format!("rule-{id}"),
        expression: expression.to_string(),
        target: "https://hooks.acme.com/vpsman".to_string(),
        body_template: String::new(),
        cooldown_secs: 30,
    };
    let first_page = vec![rule(1, "(tag:edge"), rule(2, "status = online")];
    let final_page = vec![rule(3, "interval.30sec && tag:edge")];

    assert_eq!(
        next_enabled_rule_cursor(&first_page, 2),
        Some(Uuid::from_u128(2))
    );
    assert_eq!(next_enabled_rule_cursor(&final_page, 2), None);

    let all_rules = first_page.into_iter().chain(final_page).collect::<Vec<_>>();
    assert!(all_rules.iter().any(|rule| {
        validated_rule_expression(rule).is_ok_and(|expression| {
            expression_referenced_events(&expression).contains("interval.30sec")
        })
    }));
}

#[tokio::test]
async fn postgres_enabled_rule_pages_are_all_loaded_before_atomic_event_evaluation() {
    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    let mut expected = Vec::new();
    for index in 0..5 {
        expected.push(
            insert_webhook_test_rule(
                &db.pool,
                &format!("paginated-rule-{index}"),
                "telemetry.rollup",
            )
            .await,
        );
    }
    expected.sort_unstable();

    let rules = list_enabled_rules(&db.pool, 2).await.unwrap();
    let actual = rules.iter().map(|rule| rule.id).collect::<Vec<_>>();
    assert_eq!(actual, expected, "a page bound must not omit enabled rules");

    db.cleanup().await;
}

#[test]
fn persisted_rule_validation_reports_expression_and_template_errors() {
    let rule = |expression: &str, body_template: &str| RuleRow {
        id: Uuid::from_u128(1),
        actor_id: None,
        name: "rule".to_string(),
        expression: expression.to_string(),
        target: "https://hooks.acme.com/vpsman".to_string(),
        body_template: body_template.to_string(),
        cooldown_secs: 30,
    };

    let expression_error = validated_rule_expression(&rule("(tag:edge", ""))
        .unwrap_err()
        .to_string();
    assert!(expression_error.starts_with("invalid webhook rule expression:"));

    let template_error =
        validated_rule_expression(&rule("tag:edge", "[if alert.triggered]missing end"))
            .unwrap_err()
            .to_string();
    assert!(template_error.starts_with("invalid webhook rule template:"));

    assert!(validated_rule_expression(&rule(
        "interval.30sec && tag:edge",
        "{rule.name} {event.kind}"
    ))
    .is_ok());
}

#[test]
fn configuration_failure_identity_changes_only_with_material_configuration() {
    let mut rule = RuleRow {
        id: Uuid::from_u128(1),
        actor_id: None,
        name: "rule".to_string(),
        expression: "(tag:edge".to_string(),
        target: "https://hooks.acme.com/vpsman".to_string(),
        body_template: String::new(),
        cooldown_secs: 30,
    };
    let original = rule_configuration_failure_event_id(&rule);
    assert_eq!(original, rule_configuration_failure_event_id(&rule));

    rule.name = "renamed rule".to_string();
    assert_eq!(original, rule_configuration_failure_event_id(&rule));

    rule.expression = "(tag:core".to_string();
    assert_ne!(original, rule_configuration_failure_event_id(&rule));
}

#[test]
fn webhook_signature_uses_payload_bytes() {
    let signature = webhook_signature("secret", br#"{"hello":"world"}"#).unwrap();
    assert_eq!(
        signature,
        "sha256=2677ad3e7c090b2fa2c0fb13020d66d5420879b8316eb356a2d60fb9073bc778"
    );
}

#[test]
fn candidate_uses_interval_predicate_and_aggregates_matches() {
    let rule = RuleRow {
        id: Uuid::nil(),
        actor_id: None,
        name: "edge interval".to_string(),
        expression: "interval.30sec && tag:edge".to_string(),
        target: "https://hooks.acme.com/vpsman".to_string(),
        body_template: "{event.kind} {vps.id}".to_string(),
        cooldown_secs: 30,
    };
    let vps_rows = vec![
        VpsRow {
            id: "edge-a".to_string(),
            display_name: "edge-a".to_string(),
            status: "online".to_string(),
            tags: vec!["edge".to_string()],
            registration_ip: None,
            last_ip: None,
            last_seen_at: None,
            internal_build_number: 1,
            stale_since: None,
            stale_reason: None,
            capabilities: json!({}),
            retained_tombstone: false,
            vps_rules: VpsRuleContext::default(),
        },
        VpsRow {
            id: "core-a".to_string(),
            display_name: "core-a".to_string(),
            status: "online".to_string(),
            tags: vec!["core".to_string()],
            registration_ip: None,
            last_ip: None,
            last_seen_at: None,
            internal_build_number: 1,
            stale_since: None,
            stale_reason: None,
            capabilities: json!({}),
            retained_tombstone: false,
            vps_rules: VpsRuleContext::default(),
        },
    ];
    let candidate =
        delivery_candidate_for_rule(&rule, "interval.30sec", "interval.30sec:1", &vps_rows, 1)
            .unwrap()
            .unwrap();
    assert_eq!(candidate.matched_vps.len(), 1);
    assert_eq!(candidate.message, "interval.30sec edge-a");
}

#[test]
fn candidate_can_match_vps_rules_without_exposing_them_in_payload() {
    let rule = RuleRow {
        id: Uuid::nil(),
        actor_id: None,
        name: "rule scoped interval".to_string(),
        expression: "interval.30sec && vps.rules:traffic.reset_day >= 15".to_string(),
        target: "https://hooks.acme.com/vpsman".to_string(),
        body_template: "{event.kind} {vps.id}".to_string(),
        cooldown_secs: 30,
    };
    let mut vps_rules = VpsRuleContext::default();
    insert_persisted_vps_rule(
        &mut vps_rules,
        "traffic.reset_day".to_string(),
        "15 00:00".to_string(),
        json!({"day": 15, "hour": 0}),
    )
    .unwrap();
    let vps_rows = vec![VpsRow {
        id: "edge-a".to_string(),
        display_name: "edge-a".to_string(),
        status: "online".to_string(),
        tags: vec!["edge".to_string()],
        registration_ip: None,
        last_ip: None,
        last_seen_at: None,
        internal_build_number: 1,
        stale_since: None,
        stale_reason: None,
        capabilities: json!({}),
        retained_tombstone: false,
        vps_rules,
    }];

    let candidate =
        delivery_candidate_for_rule(&rule, "interval.30sec", "interval.30sec:1", &vps_rows, 1)
            .unwrap()
            .unwrap();
    assert_eq!(candidate.matched_vps.len(), 1);
    assert_eq!(candidate.payload["matched_vps"][0].get("vps_rules"), None);
    assert!(insert_persisted_vps_rule(
        &mut VpsRuleContext::default(),
        "network.port_speed".to_string(),
        "not-a-speed".to_string(),
        json!({}),
    )
    .is_err());
}

#[test]
fn policy_alert_event_uses_event_roots_without_webhook_rule_collision() {
    let rule = RuleRow {
        id: Uuid::nil(),
        actor_id: None,
        name: "delivery-rule".to_string(),
        expression: "alert.triggered && alert.category:traffic && traffic.cycle_percent >= 80 && policy.name = monthly && policy_rule.name = quota-80 && policy_rule.trigger_meta_condition.window_seconds = 0".to_string(),
        target: "https://hooks.acme.com/vpsman".to_string(),
        body_template:
            "{rule.name} {policy.name} {policy_rule.name} {traffic.cycle_percent}".to_string(),
        cooldown_secs: 30,
    };
    let event = EventRow {
        id: Uuid::from_u128(7),
        actor_id: None,
        kind: "alert.triggered".to_string(),
        event_id: "policy-alert:test".to_string(),
        event_predicates: vec![
            "alert.triggered".to_string(),
            "alert.category:traffic".to_string(),
        ],
        subject_client_ids: vec!["edge-a".to_string()],
        payload: json!({
            "event": {"kind": "alert.triggered"},
            "alert": {"category": "traffic"},
            "policy": {"name": "monthly"},
            "policy_rule": {
                "name": "quota-80",
                "trigger_meta_condition": {"kind": "immediate", "window_seconds": 0}
            },
            "traffic": {"cycle_percent": 82.0},
        }),
        occurred_at_unix: 1,
    };
    let vps_rows = vec![VpsRow {
        id: "edge-a".to_string(),
        display_name: "edge-a".to_string(),
        status: "online".to_string(),
        tags: vec!["edge".to_string()],
        registration_ip: None,
        last_ip: None,
        last_seen_at: None,
        internal_build_number: 1,
        stale_since: None,
        stale_reason: None,
        capabilities: json!({}),
        retained_tombstone: false,
        vps_rules: VpsRuleContext::default(),
    }];

    let candidate = event_candidate_for_rule(&rule, &event, &vps_rows)
        .unwrap()
        .unwrap();
    assert_eq!(candidate.message, "delivery-rule monthly quota-80 82.0");
    assert_eq!(candidate.payload["rule"]["name"], "delivery-rule");
    assert_eq!(candidate.payload["policy_rule"]["name"], "quota-80");
    assert_eq!(candidate.payload["policy"]["name"], "monthly");
    assert_eq!(candidate.payload["traffic"]["cycle_percent"], 82.0);
}

#[test]
fn generic_alert_lifecycle_edges_preserve_payload_and_subject_identity() {
    let vps_rows = vec![VpsRow {
        id: "edge-a".to_string(),
        display_name: "edge-a".to_string(),
        status: "online".to_string(),
        tags: vec!["edge".to_string()],
        registration_ip: None,
        last_ip: None,
        last_seen_at: None,
        internal_build_number: 1,
        stale_since: None,
        stale_reason: None,
        capabilities: json!({}),
        retained_tombstone: false,
        vps_rules: VpsRuleContext::default(),
    }];
    let episode_id = Uuid::from_u128(17);
    let lifecycle_event = |kind: &str, state: &str, suffix: &str| EventRow {
        id: Uuid::from_u128(if state == "triggered" { 18 } else { 19 }),
        actor_id: None,
        kind: kind.to_string(),
        event_id: format!("fleet-alert:{episode_id}:{suffix}"),
        event_predicates: vec![
            kind.to_string(),
            "alert.category:job".to_string(),
            "alert.severity:critical".to_string(),
        ],
        subject_client_ids: vec!["edge-a".to_string()],
        payload: json!({
            "event": {
                "kind": kind,
                "id": format!("fleet-alert:{episode_id}:{suffix}"),
                "occurred_at": "2026-08-18T12:00:00Z",
            },
            "alert": {
                "id": "job:failed:job-1",
                "episode_id": episode_id,
                "record_kind": "event",
                "producer_kind": "job",
                "trigger_generation": 1,
                "lifecycle_state": state,
                "severity": "critical",
                "category": "job",
                "target_kind": "job",
                "target_id": "job-1",
                "client_id": "edge-a",
                "title": "Job failed",
                "detail": "exit status 1",
                "status": "failed",
                "triggered_at": "2026-08-18T11:59:00Z",
                "last_confirmed_at": "2026-08-18T11:59:00Z",
                "resolved_at": if state == "resolved" {
                    Some("2026-08-18T12:00:00Z")
                } else {
                    None
                },
                "resolution_reason": if state == "resolved" {
                    Some("operator_resolved")
                } else {
                    None
                },
                "evidence": {"job_id": "job-1"},
            },
        }),
        occurred_at_unix: 1,
    };
    let candidate = |expression: &str, event: &EventRow| {
        event_candidate_for_rule(
            &RuleRow {
                id: Uuid::nil(),
                actor_id: None,
                name: "generic-alert-delivery".to_string(),
                expression: expression.to_string(),
                target: "https://hooks.acme.com/vpsman".to_string(),
                body_template: "{event.kind} {alert.id} {alert.lifecycle_state}".to_string(),
                cooldown_secs: 300,
            },
            event,
            &vps_rows,
        )
        .unwrap()
        .unwrap()
    };

    let triggered_event = lifecycle_event("alert.triggered", "triggered", "triggered");
    let triggered = candidate(
        "alert.triggered && alert.category:job && alert.record_kind = event",
        &triggered_event,
    );
    assert_eq!(triggered.event_kind, "alert.triggered");
    assert_eq!(triggered.event_id, triggered_event.event_id);
    assert_eq!(
        triggered.payload["alert"]["episode_id"],
        episode_id.to_string()
    );
    assert_eq!(triggered.payload["alert"]["lifecycle_state"], "triggered");
    assert_eq!(triggered.payload["matched_vps"][0]["id"], "edge-a");

    let resolved_event = lifecycle_event("alert.resolved", "resolved", "resolved");
    let resolved = candidate(
        "alert.resolved && alert.category:job && alert.record_kind = event",
        &resolved_event,
    );
    assert_eq!(resolved.event_kind, "alert.resolved");
    assert_eq!(resolved.event_id, resolved_event.event_id);
    assert_eq!(resolved.payload["alert"]["lifecycle_state"], "resolved");
    assert_eq!(
        resolved.payload["alert"]["resolution_reason"],
        "operator_resolved"
    );
}

#[test]
fn subjectless_generic_alert_edges_match_only_event_and_alert_context() {
    let mut vps_rules = VpsRuleContext::default();
    insert_persisted_vps_rule(
        &mut vps_rules,
        "traffic.reset_day".to_string(),
        "15 00:00".to_string(),
        json!({"day": 15, "hour": 0}),
    )
    .unwrap();
    let visible_vps = vec![
        VpsRow {
            id: "edge-a".to_string(),
            display_name: "edge-a".to_string(),
            status: "online".to_string(),
            tags: vec!["edge".to_string()],
            registration_ip: None,
            last_ip: None,
            last_seen_at: None,
            internal_build_number: 1,
            stale_since: None,
            stale_reason: None,
            capabilities: json!({}),
            retained_tombstone: false,
            vps_rules,
        },
        VpsRow {
            id: "untagged-a".to_string(),
            display_name: "untagged-a".to_string(),
            status: "online".to_string(),
            tags: Vec::new(),
            registration_ip: None,
            last_ip: None,
            last_seen_at: None,
            internal_build_number: 1,
            stale_since: None,
            stale_reason: None,
            capabilities: json!({}),
            retained_tombstone: false,
            vps_rules: VpsRuleContext::default(),
        },
    ];
    let event = EventRow {
        id: Uuid::from_u128(20),
        actor_id: None,
        kind: "alert.triggered".to_string(),
        event_id: "fleet-alert:global-job:triggered".to_string(),
        event_predicates: vec![
            "alert.triggered".to_string(),
            "alert.category:job".to_string(),
            "alert.severity:critical".to_string(),
        ],
        subject_client_ids: Vec::new(),
        payload: json!({
            "event": {
                "kind": "alert.triggered",
                "id": "fleet-alert:global-job:triggered",
            },
            "alert": {
                "id": "job:failed:global-job",
                "record_kind": "event",
                "producer_kind": "job",
                "lifecycle_state": "triggered",
                "severity": "critical",
                "category": "job",
                "target_kind": "job",
                "target_id": "global-job",
                "client_id": null,
            },
        }),
        occurred_at_unix: 1,
    };
    let candidate = |expression: &str, vps_rows: &[VpsRow]| {
        event_candidate_for_rule(
            &RuleRow {
                id: Uuid::nil(),
                actor_id: None,
                name: "global-alert".to_string(),
                expression: expression.to_string(),
                target: "https://hooks.acme.com/vpsman".to_string(),
                body_template: "{event.kind} {alert.id}".to_string(),
                cooldown_secs: 300,
            },
            &event,
            vps_rows,
        )
        .unwrap()
    };

    let empty_fleet = candidate(
        "alert.triggered && alert.category:job && alert.record_kind = event",
        &[],
    )
    .unwrap();
    assert!(empty_fleet.matched_vps.is_empty());
    assert_eq!(empty_fleet.payload["alert"]["client_id"], Value::Null);

    let visible_fleet = candidate(
        "alert.triggered && alert.category:job && alert.record_kind = event",
        &visible_vps,
    )
    .unwrap();
    assert!(visible_fleet.matched_vps.is_empty());
    assert_eq!(visible_fleet.payload["matched_vps"], json!([]));

    for expression in [
        "alert.triggered && vps.id = edge-a",
        "alert.triggered && status = online",
        "alert.triggered && tag:edge",
        "alert.triggered && vps.rules:traffic.reset_day >= 1",
        "alert.triggered && edge",
        "alert.triggered && untagged",
        "alert.triggered && !(status = offline)",
    ] {
        assert!(
            candidate(expression, &visible_vps).is_none(),
            "VPS-dependent expression must fail closed: {expression}"
        );
    }
}

async fn insert_claimed_client_alert_webhook_delivery(
    pool: &PgPool,
    rule_id: Uuid,
    delivery_id: Uuid,
    lease_id: Uuid,
    client_id: &str,
) {
    sqlx::query(
        r#"
        INSERT INTO webhook_rule_deliveries (
            id, rule_id, rule_name, event_kind, event_id, status,
            target, dedupe_key, payload, matched_vps, message,
            cooldown_until_unix, delivery_lease_id, delivery_lease_until
        ) VALUES (
            $1, $2, 'webhook-alert-fence', 'alert.triggered', $3,
            'in_progress', 'https://hooks.example.invalid/vpsman', $4,
            '{}'::jsonb, jsonb_build_array(jsonb_build_object('id',$5::text)),
            'alert', 0, $6, now() + interval '60 seconds'
        )
        "#,
    )
    .bind(delivery_id)
    .bind(rule_id)
    .bind(format!("fleet-alert:{client_id}:{delivery_id}:triggered"))
    .bind(format!("webhook-alert-fence:{delivery_id}"))
    .bind(client_id)
    .bind(lease_id)
    .execute(pool)
    .await
    .unwrap();
}

async fn insert_pending_client_alert_lifecycle_source(
    pool: &PgPool,
    client_id: &str,
) -> (Uuid, String, i64, Value) {
    let episode_id = Uuid::new_v4();
    let public_id = format!("pending-alert:{episode_id}");
    sqlx::query(
        r#"
        INSERT INTO alert_episodes (
            id, public_id, producer_kind, natural_key, record_kind,
            trigger_generation, trigger_severity, trigger_category, severity,
            category, target_kind, target_id, client_id, title, detail,
            source_status, evidence, lifecycle_state, triggered_at,
            last_confirmed_at, policy_group_id, policy_rule_id,
            policy_rule_version, policy_rule_kind, policy_group_name,
            policy_rule_name, policy_rule_system_seed_key
        )
        SELECT
            $1, $2, rule.evidence_source, $3, 'condition', 1,
            'warning', 'agent_status', 'warning', 'agent_status',
            'agent', $3, $3, 'Pending client alert', 'test pending edge',
            'offline', '{}'::jsonb, 'triggered', now(), now(),
            rule.group_id, rule.id, rule.rule_version, rule.rule_kind,
            policy.name, rule.name, rule.system_seed_key
        FROM policy_rules rule
        JOIN policy_groups policy ON policy.id=rule.group_id
        WHERE rule.id='d1000000-0000-4000-8000-000000000003'
        "#,
    )
    .bind(episode_id)
    .bind(&public_id)
    .bind(client_id)
    .execute(pool)
    .await
    .unwrap();
    let event_id = format!("fleet-alert:{episode_id}:triggered");
    let event_seq: i64 = sqlx::query_scalar("SELECT nextval('alert_lifecycle_event_seq')")
        .fetch_one(pool)
        .await
        .unwrap();
    let payload = json!({
        "event": {"kind": "alert.triggered", "id": &event_id},
        "alert": {
            "id": &public_id,
            "episode_id": episode_id,
            "record_kind": "condition",
            "lifecycle_state": "triggered",
            "trigger_generation": 1,
            "severity": "warning",
            "category": "agent_status",
            "client_id": client_id,
        },
    });
    sqlx::query(
        r#"
        INSERT INTO alert_lifecycle_events (
            event_seq, id, episode_id, trigger_generation, edge_kind,
            event_id, event_predicates, subject_client_ids, payload,
            occurred_at
        ) VALUES (
            $1,$2,$3,1,'alert.triggered',$4,
            ARRAY['alert.triggered','alert.category:agent_status','alert.severity:warning'],
            ARRAY[$5]::text[],$6,now()
        )
        "#,
    )
    .bind(event_seq)
    .bind(Uuid::new_v4())
    .bind(episode_id)
    .bind(&event_id)
    .bind(client_id)
    .bind(SqlJson(&payload))
    .execute(pool)
    .await
    .unwrap();
    (episode_id, event_id, event_seq, payload)
}

#[allow(clippy::too_many_arguments)]
fn telemetry_event_reference(
    client_id: &str,
    gateway_id: &str,
    gateway_session_id: Uuid,
    process_incarnation_id: Uuid,
    source_telemetry_seq: u64,
    reported_observed_unix: u64,
    occurred_at: DateTime<Utc>,
    stored_metrics: &AgentMetrics,
) -> EventRow {
    // Keep this independent from the cursor-direct implementation so field
    // parity catches accidental projection drift.
    let mut metrics = stored_metrics.clone();
    metrics.observed_unix = reported_observed_unix;
    let sum_u64 = |values: Vec<u64>| {
        values
            .into_iter()
            .fold(0_u128, |total, value| total.saturating_add(value as u128))
            .min(i64::MAX as u128) as i64
    };
    let disk = metrics
        .has_persistent_block_filesystem_disk_sample()
        .then(|| {
            (
                sum_u64(metrics.disks.iter().map(|disk| disk.total_bytes).collect()),
                sum_u64(
                    metrics
                        .disks
                        .iter()
                        .map(|disk| disk.available_bytes)
                        .collect(),
                ),
            )
        });
    let default_networks = metrics
        .networks
        .iter()
        .filter(|network| {
            (network.interface.starts_with('e') || network.interface.starts_with('w'))
                && !metrics
                    .tunnels
                    .iter()
                    .any(|tunnel| tunnel.interface == network.interface)
        })
        .collect::<Vec<_>>();
    let default_tunnels = metrics
        .tunnels
        .iter()
        .map(|tunnel| {
            let mut tunnel = serde_json::to_value(tunnel).unwrap();
            let object = tunnel.as_object_mut().unwrap();
            for field in [
                "rx_bytes",
                "tx_bytes",
                "traffic_source",
                "traffic_status",
                "traffic_reason",
                "traffic_checked_unix",
            ] {
                object.remove(field);
            }
            tunnel
        })
        .collect::<Vec<_>>();
    let network_rx = sum_u64(
        default_networks
            .iter()
            .map(|network| network.rx_bytes)
            .collect(),
    );
    let network_tx = sum_u64(
        default_networks
            .iter()
            .map(|network| network.tx_bytes)
            .collect(),
    );
    let mut predicates = vec!["telemetry.rollup".to_string()];
    if !metrics.networks.is_empty() {
        predicates.push("telemetry.network_rate".to_string());
    }
    if !metrics.tunnels.is_empty() {
        predicates.push("telemetry.tunnel".to_string());
    }
    if !metrics.tunnel_reachability.is_empty() {
        predicates.push("network.reachability".to_string());
    }
    predicates.sort();
    predicates.dedup();
    let event_id = format!(
        "telemetry:{client_id}:{gateway_session_id}:{process_incarnation_id}:{source_telemetry_seq}"
    );
    EventRow {
        id: Uuid::nil(),
        actor_id: None,
        kind: "telemetry.rollup".to_string(),
        event_id: event_id.clone(),
        event_predicates: predicates.clone(),
        subject_client_ids: vec![client_id.to_string()],
        payload: json!({
            "event": {
                "kind": "telemetry.rollup",
                "id": &event_id,
                "predicates": &predicates,
            },
            "telemetry": {
                "client_id": client_id,
                "gateway_id": gateway_id,
                "observed_unix": metrics.observed_unix,
                "hostname": &metrics.hostname,
                "uptime_secs": metrics.uptime_secs,
                "disk_collection_available": disk.is_some(),
                "disk_total_bytes": disk.map(|(total, _)| total),
                "disk_available_bytes": disk.map(|(_, available)| available),
                "network_rx_bytes": network_rx,
                "network_tx_bytes": network_tx,
                "network_count": default_networks.len(),
                "tunnel_count": metrics.tunnels.len(),
                "networks": &default_networks,
                "tunnels": &default_tunnels,
            },
        }),
        occurred_at_unix: occurred_at.timestamp(),
    }
}

async fn insert_webhook_test_client(pool: &PgPool, client_id: &str, status: &str, hidden: bool) {
    sqlx::query(
        r#"
        INSERT INTO clients (
            id, display_name, public_key, status, internal_build_number,
            capabilities, hidden_at
        )
        VALUES ($1, $1, decode('', 'hex'), $2, 1, '{}'::jsonb,
            CASE WHEN $3 THEN now() ELSE NULL END)
        "#,
    )
    .bind(client_id)
    .bind(status)
    .bind(hidden)
    .execute(pool)
    .await
    .unwrap();
}

async fn insert_webhook_test_rule(pool: &PgPool, name: &str, expression: &str) -> Uuid {
    let rule_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO webhook_rules (
            id, name, enabled, expression, target, body_template, cooldown_secs
        )
        VALUES ($1, $2, TRUE, $3, 'https://hooks.example.invalid/vpsman', '', 0)
        "#,
    )
    .bind(rule_id)
    .bind(name)
    .bind(expression)
    .execute(pool)
    .await
    .unwrap();
    rule_id
}

#[allow(clippy::too_many_arguments)]
async fn insert_telemetry_projection_sample(
    pool: &PgPool,
    client_id: &str,
    accepted_seq: i64,
    accepted_at: DateTime<Utc>,
    gateway_id: &str,
    gateway_session_id: Uuid,
    process_incarnation_id: Uuid,
    source_telemetry_seq: u64,
    reported_observed_unix: i64,
    metrics: &AgentMetrics,
) {
    let network_admission_mask = test_ordinal_mask(
        metrics
            .networks
            .iter()
            .map(|network| {
                (network.interface.starts_with('e') || network.interface.starts_with('w'))
                    && !metrics
                        .tunnels
                        .iter()
                        .any(|tunnel| tunnel.interface == network.interface)
            })
            .collect::<Vec<_>>(),
    );
    let tunnel_admission_mask = test_ordinal_mask(vec![false; metrics.tunnels.len()]);
    insert_telemetry_projection_sample_with_masks(
        pool,
        client_id,
        accepted_seq,
        accepted_at,
        gateway_id,
        gateway_session_id,
        process_incarnation_id,
        source_telemetry_seq,
        reported_observed_unix,
        metrics,
        &network_admission_mask,
        &tunnel_admission_mask,
    )
    .await;
}

fn test_ordinal_mask(admission: Vec<bool>) -> Vec<u8> {
    let mut mask = vec![0_u8; admission.len().div_ceil(8)];
    for (ordinal, admitted) in admission.into_iter().enumerate() {
        if admitted {
            mask[ordinal / 8] |= 1_u8 << (ordinal % 8);
        }
    }
    mask
}

#[allow(clippy::too_many_arguments)]
async fn insert_telemetry_projection_sample_with_masks(
    pool: &PgPool,
    client_id: &str,
    accepted_seq: i64,
    accepted_at: DateTime<Utc>,
    gateway_id: &str,
    gateway_session_id: Uuid,
    process_incarnation_id: Uuid,
    source_telemetry_seq: u64,
    reported_observed_unix: i64,
    metrics: &AgentMetrics,
    network_admission_mask: &[u8],
    tunnel_admission_mask: &[u8],
) {
    let bounded = |value: u64| value.min(i64::MAX as u64) as i64;
    let disk = metrics
        .has_persistent_block_filesystem_disk_sample()
        .then(|| {
            (
                metrics
                    .disks
                    .iter()
                    .fold(0_u128, |total, disk| {
                        total.saturating_add(disk.total_bytes as u128)
                    })
                    .min(i64::MAX as u128) as i64,
                metrics
                    .disks
                    .iter()
                    .fold(0_u128, |total, disk| {
                        total.saturating_add(disk.available_bytes as u128)
                    })
                    .min(i64::MAX as u128) as i64,
            )
        });
    let (tcp_sockets, udp_sockets) = metrics
        .connections
        .as_ref()
        .map(|connections| (bounded(connections.tcp), bounded(connections.udp)))
        .unwrap_or((i64::MAX, i64::MAX));
    let mut tx = pool.begin().await.unwrap();
    let updated = sqlx::query(
        r#"
        UPDATE telemetry_projection_heads
        SET accepted_seq=$2, projected_seq=$2,
            accepted_at=GREATEST(accepted_at, $3::timestamptz)
        WHERE client_id=$1 AND accepted_seq=$2 - 1
        "#,
    )
    .bind(client_id)
    .bind(accepted_seq)
    .bind(accepted_at)
    .execute(&mut *tx)
    .await
    .unwrap();
    assert_eq!(updated.rows_affected(), 1);
    sqlx::query(
        r#"
        INSERT INTO telemetry_samples (
            id, client_id, observed_at,
            cpu_utilization_ratio, cpu_cores,
            cpu_load_1, cpu_load_5, cpu_load_15,
            memory_total_bytes, memory_available_bytes,
            swap_total_bytes, swap_available_bytes,
            disk_total_bytes, disk_available_bytes,
            tcp_sockets, udp_sockets, payload,
            accepted_seq, accepted_at, source_gateway_id,
            source_gateway_session_id, source_process_incarnation_id,
            source_telemetry_seq, reported_observed_unix,
            network_admission_mask, tunnel_admission_mask
        ) VALUES (
            $1, $2, to_timestamp($3::double precision),
            $4, $5, $6, $7, $8,
            $9, $10, $11, $12, $13, $14, $15, $16, $17,
            $18, $19, $20, $21, $22, $23, $24, $25, $26
        )
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(client_id)
    .bind(metrics.observed_unix.min(i64::MAX as u64) as f64)
    .bind(metrics.cpu.utilization_ratio)
    .bind(i32::from(metrics.cpu.cores))
    .bind(metrics.cpu.load.one)
    .bind(metrics.cpu.load.five)
    .bind(metrics.cpu.load.fifteen)
    .bind(bounded(metrics.memory.total_bytes))
    .bind(bounded(metrics.memory.available_bytes))
    .bind(metrics.memory.swap_total_bytes.map(bounded))
    .bind(metrics.memory.swap_available_bytes.map(bounded))
    .bind(disk.map(|value| value.0))
    .bind(disk.map(|value| value.1))
    .bind(tcp_sockets)
    .bind(udp_sockets)
    .bind(SqlJson(metrics))
    .bind(accepted_seq)
    .bind(accepted_at)
    .bind(gateway_id)
    .bind(gateway_session_id)
    .bind(process_incarnation_id)
    .bind(bounded(source_telemetry_seq))
    .bind(reported_observed_unix)
    .bind(network_admission_mask)
    .bind(tunnel_admission_mask)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
}
