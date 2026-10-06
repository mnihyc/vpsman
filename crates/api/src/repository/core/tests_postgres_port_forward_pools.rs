use super::*;
use vpsman_common::{
    PortForwardCapability, PortForwardCapabilityStatus, PortForwardPoolCapabilities,
};

fn pool() -> Value {
    json!({"incoming":[{"start":18443,"end":18444}],"strategy":"round_robin", "upstreams":[
        {"id":Uuid::new_v4(),"target_ip":"192.0.2.1","target_hostname":"BACKEND.Example.","ports":{"start":8000,"end":8002},"weight":2,"role":"primary","enabled":true},
        {"id":Uuid::new_v4(),"target_ip":"192.0.2.2","ports":{"start":9000,"end":9000},"weight":1,"role":"primary","enabled":true}
    ]})
}

async fn enable_pool_capability(db: &PgReliabilityTestDb, client: &str) {
    let capability = PortForwardCapability {
        schema_version: 3,
        status: PortForwardCapabilityStatus::Supported,
        supported_modes: vec![
            vpsman_common::PortForwardMode::Dnat,
            vpsman_common::PortForwardMode::CustomAdapter,
        ],
        pool: Some(PortForwardPoolCapabilities::native()),
        ..Default::default()
    };
    sqlx::query("UPDATE clients SET capabilities=jsonb_set(capabilities,'{port_forwarding}',$2) WHERE id=$1")
        .bind(client).bind(sqlx::types::Json(capability)).execute(&db.pool).await.unwrap();
}

#[tokio::test]
async fn postgres_port_forward_reapply_routes_produce_work_without_a_source_mutation() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let (operator, headers) = postgres_operator_session(&db.repo, "pool-reapply").await;
    let state = postgres_app_state(&db);
    let mut items = Vec::new();
    for (index, client) in ["reapply-a", "reapply-a", "reapply-b"]
        .into_iter()
        .enumerate()
    {
        if index != 1 {
            insert_client(&db.pool, client, Some(Uuid::new_v4())).await;
            enable_pool_capability(&db, client).await;
        }
        let mut request = json!({"client_id":client,"name":format!("rule-{index}"),"protocol":"tcp","mappings":[],"pool":pool(),"enabled":true});
        if index == 1 {
            request["pool"] = Value::Null;
            request["target_ip"] = json!("192.0.2.3");
            request["mappings"] = json!(pair_port_expressions("18445", "443").unwrap());
        }
        let rule = db
            .repo
            .create_port_forward_rule(&serde_json::from_value(request).unwrap(), &operator)
            .await
            .unwrap();
        items.push(PortForwardBulkItem {
            id: rule.id,
            expected_revision: rule.revision,
        });
    }
    // Source-triggered work has already been consumed before the operator presses Reapply.
    sqlx::query("DELETE FROM client_runtime_config_reconcile_work")
        .execute(&db.pool)
        .await
        .unwrap();
    for client in ["reapply-a", "reapply-b"] {
        let mut config = vpsman_common::AgentRuntimeConfig::default();
        config.network.port_forwarding = db
            .repo
            .port_forwarding_config_for_client(client)
            .await
            .unwrap();
        let hash = vpsman_common::runtime_config_content_hash(&config).unwrap();
        sqlx::query("INSERT INTO client_runtime_config_apply_state (client_id, applied_version, applied_content_hash, applied_config) VALUES ($1,1,$2,$3)")
            .bind(client).bind(hash).bind(SqlJson(config)).execute(&db.pool).await.unwrap();
    }
    let response = crate::routes_port_forwarding::reapply_port_forward_rule(
        State(state.clone()),
        headers.clone(),
        axum::extract::Path(items[0].id),
        Json(crate::model_port_forwarding::PortForwardMutationRequest {
            expected_revision: items[0].expected_revision,
            confirmed: true,
            reason: None,
        }),
    )
    .await
    .unwrap();
    assert_eq!(response.0.sync.status, "queued");
    let work: Vec<(String, String)> = sqlx::query_as(
        "SELECT client_id, reason FROM client_runtime_config_reconcile_work ORDER BY client_id",
    )
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        work,
        [("reapply-a".into(), "port_forward_table_reapply".into())]
    );
    let claim = db
        .repo
        .claim_runtime_config_reconciliation(Some("reapply-a"), 30)
        .await
        .unwrap()
        .unwrap();
    let hash: String = sqlx::query_scalar("SELECT applied_content_hash FROM client_runtime_config_apply_state WHERE client_id='reapply-a'").fetch_one(&db.pool).await.unwrap();
    assert_eq!(
        claim.acknowledge_if_content_current(&hash).await.unwrap(),
        None,
        "Reapply must reach the agent even with identical applied content"
    );
    sqlx::query("DELETE FROM client_runtime_config_reconcile_work")
        .execute(&db.pool)
        .await
        .unwrap();
    let stale = crate::routes_port_forwarding::reapply_port_forward_rule(
        State(state.clone()),
        headers.clone(),
        axum::extract::Path(items[0].id),
        Json(crate::model_port_forwarding::PortForwardMutationRequest {
            expected_revision: 0,
            confirmed: true,
            reason: None,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(stale.code, "port_forward_rule_snapshot_stale");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM client_runtime_config_reconcile_work")
            .fetch_one(&db.pool)
            .await
            .unwrap(),
        0
    );
    let response = crate::routes_port_forwarding::bulk_mutate_port_forward_rules(
        State(state),
        headers,
        Json(crate::model_port_forwarding::PortForwardBulkRequest {
            action: PortForwardBulkAction::Reapply,
            items: items.clone(),
            confirmed: true,
            reason: None,
        }),
    )
    .await
    .unwrap();
    assert_eq!(response.0.sync.len(), 2);
    let work: Vec<(String, String)> = sqlx::query_as(
        "SELECT client_id, reason FROM client_runtime_config_reconcile_work ORDER BY client_id",
    )
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        work,
        [
            ("reapply-a".into(), "port_forward_bulk_reapply".into()),
            ("reapply-b".into(), "port_forward_bulk_reapply".into())
        ]
    );
    for client in ["reapply-a", "reapply-b"] {
        let claim = db
            .repo
            .claim_runtime_config_reconciliation(Some(client), 30)
            .await
            .unwrap()
            .unwrap();
        let hash: String = sqlx::query_scalar(
            "SELECT applied_content_hash FROM client_runtime_config_apply_state WHERE client_id=$1",
        )
        .bind(client)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(
            claim.acknowledge_if_content_current(&hash).await.unwrap(),
            None
        );
    }
    for reason in [
        "agent_reconnect_authoritative_sync",
        "agent_reconnect_authoritative_port_forwarding_sync",
        "agent_reconnect_port_forwarding_sync",
        "agent_reconnect_runtime_tunnels_sync",
    ] {
        db.repo
            .enqueue_runtime_config_reconciliations(&["reapply-a".into()], reason, None)
            .await
            .unwrap();
        let claim = db
            .repo
            .claim_runtime_config_reconciliation(Some("reapply-a"), 30)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            claim.acknowledge_if_content_current(&hash).await.unwrap(),
            None,
            "{reason}"
        );
    }
    for item in items {
        assert_eq!(
            db.repo
                .get_port_forward_rule(item.id)
                .await
                .unwrap()
                .unwrap()
                .revision,
            item.expected_revision
        );
    }
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_port_forward_pool_roundtrip_updates_and_explicit_mapping_transition() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let operator = postgres_network_operator(&db.repo).await;
    insert_client(&db.pool, "pool-native", Some(Uuid::new_v4())).await;
    let mut payload = json!({"client_id":"pool-native","name":"pool","protocol":"tcp","mappings":[],"pool":pool(),"enabled":false});
    let saved = db
        .repo
        .create_port_forward_rule(&serde_json::from_value(payload.clone()).unwrap(), &operator)
        .await
        .unwrap();
    assert_eq!(
        saved.pool.as_ref().unwrap().upstreams[0]
            .target_hostname
            .as_deref(),
        Some("backend.example")
    );
    let error = db
        .repo
        .set_port_forward_rule_enabled(saved.id, 1, true, &operator)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("pool_agent_capability_required"));
    enable_pool_capability(&db, "pool-native").await;
    let saved = db
        .repo
        .set_port_forward_rule_enabled(saved.id, 1, true, &operator)
        .await
        .unwrap();
    let config = db
        .repo
        .port_forwarding_config_for_client("pool-native")
        .await
        .unwrap();
    assert_eq!(config.schema_version, 3);
    assert!(config.rules[0].pool.as_ref().unwrap().upstreams[0]
        .target_hostname
        .is_none());
    assert!(config.rules[0].target_ip.is_none());
    assert!(config.rules[0].mappings.is_empty());
    payload.as_object_mut().unwrap().remove("client_id");
    payload["expected_revision"] = json!(saved.revision);
    payload["enabled"] = json!(true);
    let original_pool = payload.as_object_mut().unwrap().remove("pool").unwrap();
    payload["target_ip"] = json!("192.0.2.3");
    payload["mappings"] = json!(pair_port_expressions("18443", "443").unwrap());
    let error = db
        .repo
        .update_port_forward_rule(
            saved.id,
            &serde_json::from_value(payload.clone()).unwrap(),
            &operator,
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("pool_update_required"));
    // Explicit conversion owns the change; the old omission cannot erase a pool.
    payload["pool"] = Value::Null;
    let mapped = db
        .repo
        .update_port_forward_rule(
            saved.id,
            &serde_json::from_value(payload.clone()).unwrap(),
            &operator,
        )
        .await
        .unwrap();
    assert!(mapped.pool.is_none());
    assert_eq!(
        db.repo
            .port_forwarding_config_for_client("pool-native")
            .await
            .unwrap()
            .schema_version,
        1
    );
    payload["pool"] = original_pool;
    payload["target_ip"] = Value::Null;
    payload["mappings"] = json!([]);
    payload["pool"]["upstreams"][0]["weight"] = json!(4);
    payload["expected_revision"] = json!(mapped.revision);
    let updated = db
        .repo
        .update_port_forward_rule(
            saved.id,
            &serde_json::from_value(payload).unwrap(),
            &operator,
        )
        .await
        .unwrap();
    assert_eq!(updated.pool.unwrap().upstreams[0].weight, 4);
    let loaded = db.repo.list_port_forward_rules().await.unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].revision, mapped.revision + 1);
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_port_forward_pool_capability_loss_and_corruption_do_not_block_cleanup() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let operator = postgres_network_operator(&db.repo).await;
    let client = "pool-cleanup";
    insert_client(&db.pool, client, Some(Uuid::new_v4())).await;
    enable_pool_capability(&db, client).await;
    let request = serde_json::from_value(json!({
        "client_id":client,"name":"cleanup","protocol":"tcp",
        "mappings":[],"pool":pool(),"enabled":true
    }))
    .unwrap();
    let saved = db
        .repo
        .create_port_forward_rule(&request, &operator)
        .await
        .unwrap();
    sqlx::query("UPDATE clients SET capabilities='{}'::jsonb WHERE id=$1")
        .bind(client)
        .execute(&db.pool)
        .await
        .unwrap();
    let disabled = db
        .repo
        .set_port_forward_rule_enabled(saved.id, saved.revision, false, &operator)
        .await
        .unwrap();
    assert_eq!(disabled.pool, saved.pool);
    let error = db
        .repo
        .bulk_mutate_port_forward_rules(
            PortForwardBulkAction::Enable,
            &[PortForwardBulkItem {
                id: saved.id,
                expected_revision: disabled.revision,
            }],
            None,
            &operator,
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("pool_agent_capability_required"));
    let unchanged = db
        .repo
        .get_port_forward_rule(saved.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(unchanged.revision, disabled.revision);
    assert!(!unchanged.enabled);
    assert_eq!(unchanged.pool, saved.pool);

    // Corrupt execution data must not be needed to retire a rule or generate
    // its native cleanup request. Fresh absence, rather than disabling, retires it.
    sqlx::query("UPDATE port_forward_rules SET pool='{}'::jsonb WHERE id=$1")
        .bind(saved.id)
        .execute(&db.pool)
        .await
        .unwrap();
    let error = db
        .repo
        .port_forward_rule_configuration_error(saved.id)
        .await
        .unwrap()
        .unwrap();
    let deleted = db
        .repo
        .delete_corrupt_port_forward_rule(saved.id, disabled.revision, None, &error, &operator)
        .await
        .unwrap();
    assert!(deleted.removal_confirmed_at.is_none());
    let config = db
        .repo
        .port_forwarding_config_for_client(client)
        .await
        .unwrap();
    assert!(config.rules.is_empty());
    assert!(config.native_cleanup_pending);
    db.repo
        .record_port_forward_runtime_snapshot(
            client,
            &PortForwardRuntimeSnapshot {
                status: PortForwardRuntimeStatus::Absent,
                owned_table_present: Some(false),
                observed_unix: 100,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(db
        .repo
        .list_port_forward_rule_items()
        .await
        .unwrap()
        .is_empty());
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_port_forward_pool_adapter_edits_keep_attached_contract_and_cleanup() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let operator = postgres_network_operator(&db.repo).await;
    insert_client(&db.pool, "pool-custom", Some(Uuid::new_v4())).await;
    enable_pool_capability(&db, "pool-custom").await;
    let command = json!({"argv":["/opt/operator/adapter","{rule_config_path}"],"max_timeout_secs":30,"max_output_bytes":16384});
    let mut definition = crate::model::UpsertNetworkAdapterDefinitionRequest {
        adapter_kind: "port_forward".into(),
        name: "pool-adapter".into(),
        description: None,
        definition: json!({"contract_version":2,"pool_capabilities":PortForwardPoolCapabilities::native(),
            "apply_command":command,"remove_command":command,"status_command":command}),
    };
    let adapter = db
        .repo
        .create_network_adapter_definition(&definition, &operator)
        .await
        .unwrap();
    let request=serde_json::from_value(json!({"client_id":"pool-custom","name":"custom","mode":"custom_adapter","protocol":"tcp", "adapter_definition_id":adapter.id,
        "mappings":[],"pool":pool(),"enabled":true,"confirmed":true})).unwrap();
    let saved = db
        .repo
        .create_port_forward_rule(&request, &operator)
        .await
        .unwrap();
    definition
        .definition
        .as_object_mut()
        .unwrap()
        .remove("pool_capabilities");
    let error = db
        .repo
        .preview_network_adapter_definition(adapter.id, &definition)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("pool_required_by_binding"));
    let disabled = db
        .repo
        .set_port_forward_rule_enabled(saved.id, saved.revision, false, &operator)
        .await
        .unwrap();
    assert!(disabled.pool.is_some());
    assert!(
        db.repo
            .preview_network_adapter_definition(adapter.id, &definition)
            .await
            .is_err(),
        "disabled drafts retain their contract"
    );
    let cleanup = db
        .repo
        .port_forwarding_config_for_client("pool-custom")
        .await
        .unwrap();
    assert!(cleanup.rules.is_empty());
    assert_eq!(cleanup.cleanup_rules[0].rule_id, saved.id);
    let removed = db
        .repo
        .delete_port_forward_rule(saved.id, disabled.revision, None, &operator)
        .await
        .unwrap();
    let cleanup = db
        .repo
        .port_forwarding_config_for_client("pool-custom")
        .await
        .unwrap();
    assert_eq!(cleanup.cleanup_rules[0].revision, removed.revision);
    db.repo
        .record_port_forward_runtime_snapshot(
            "pool-custom",
            &PortForwardRuntimeSnapshot {
                status: PortForwardRuntimeStatus::Absent,
                removed_rules: cleanup.cleanup_rules,
                observed_unix: 100,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(db.repo.list_port_forward_rules().await.unwrap().is_empty());
    db.cleanup().await;
}
