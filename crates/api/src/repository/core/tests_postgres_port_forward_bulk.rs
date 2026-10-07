use super::*;
use vpsman_common::{port_forwarding_desired_hash, PortForwardMode};

#[tokio::test]
async fn postgres_port_forward_bulk_delete_confirms_all_clients_and_modes() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let operator = postgres_network_operator(&db.repo).await;
    let adapter = db.repo.create_network_adapter_definition(
        &crate::model::UpsertNetworkAdapterDefinitionRequest {
            adapter_kind: "port_forward".into(),
            name: "bulk-cleanup".into(),
            description: None,
            definition: json!({"contract_version":2,
                "apply_command":{"argv":["/usr/bin/true","{rule_config_json}"],"max_timeout_secs":30,"max_output_bytes":16384},
                "remove_command":{"argv":["/usr/bin/true","{rule_config_json}"],"max_timeout_secs":30,"max_output_bytes":16384},
                "status_command":{"argv":["/usr/bin/true","{rule_config_json}"],"max_timeout_secs":30,"max_output_bytes":16384}}),
        }, &operator,
    ).await.unwrap();
    let clients = ["bulk-forward-a", "bulk-forward-b"];
    let mut selected = Vec::new();
    let mut survivors = Vec::new();
    for client in clients {
        insert_client(&db.pool, client, Some(Uuid::new_v4())).await;
        sqlx::query("UPDATE clients SET capabilities = $2 WHERE id = $1")
            .bind(client)
            .bind(
                json!({"port_forwarding":{"status":"supported","schema_version":3,
                "supported_modes":["dnat","redirect","custom_adapter"]}}),
            )
            .execute(&db.pool)
            .await
            .unwrap();
        // Two custom owners catch receipt truncation; native modes share one table.
        for (index, mode) in [
            "dnat",
            "redirect",
            "custom_adapter",
            "custom_adapter",
            "dnat",
        ]
        .into_iter()
        .enumerate()
        {
            let request = serde_json::from_value(json!({
                "client_id":client, "name":format!("rule-{index}"), "mode":mode,
                "protocol":"tcp", "target_ip":if mode == "redirect" {Value::Null} else {json!("192.0.2.80")},
                "address_family":if mode == "redirect" {json!("both")} else {Value::Null},
                "adapter_definition_id":if mode == "custom_adapter" {json!(adapter.id)} else {Value::Null},
                "mappings":pair_port_expressions(&(18080 + index).to_string(), "8080").unwrap(),
                "enabled":true, "confirmed":true
            })).unwrap();
            let rule = db
                .repo
                .create_port_forward_rule(&request, &operator)
                .await
                .unwrap();
            if client == clients[1] && index == 4 {
                survivors.push(rule.id);
            } else {
                selected.push(PortForwardBulkItem {
                    id: rule.id,
                    expected_revision: rule.revision,
                });
            }
        }
    }
    let deleted = db
        .repo
        .bulk_mutate_port_forward_rules(PortForwardBulkAction::Delete, &selected, None, &operator)
        .await
        .unwrap();
    assert_eq!(deleted.len(), selected.len());
    assert!(deleted
        .iter()
        .all(|rule| rule.deleted_at.is_some() && rule.removal_confirmed_at.is_none()));
    for client in clients {
        let config = db
            .repo
            .port_forwarding_config_for_client(client)
            .await
            .unwrap();
        assert!(config.rules.iter().all(|rule| survivors.contains(&rule.id)));
        assert_eq!(config.cleanup_rules.len(), 2);
        assert!(config.native_cleanup_pending);
        let native = config
            .rules
            .iter()
            .filter(|rule| rule.mode != PortForwardMode::CustomAdapter)
            .cloned()
            .collect::<Vec<_>>();
        let native_hash = if native.is_empty() {
            String::new()
        } else {
            port_forwarding_desired_hash(&native)
        };
        // First confirmed custom removal must not hide the second pending owner.
        let mut snapshot = PortForwardRuntimeSnapshot {
            status: if config.rules.is_empty() {
                PortForwardRuntimeStatus::Absent
            } else {
                PortForwardRuntimeStatus::Applied
            },
            native_desired_hash: Some(native_hash),
            owned_table_present: Some(!native.is_empty()),
            desired_hash: (!config.desired_hash.is_empty()).then_some(config.desired_hash),
            removed_rules: vec![config.cleanup_rules[0].clone()],
            observed_unix: 100,
            ..Default::default()
        };
        db.repo
            .record_port_forward_runtime_snapshot(client, &snapshot)
            .await
            .unwrap();
        let pending = db
            .repo
            .list_port_forward_rules()
            .await
            .unwrap()
            .into_iter()
            .filter(|rule| rule.client_id == client && rule.deleted_at.is_some())
            .collect::<Vec<_>>();
        assert_eq!(
            pending.len(),
            1,
            "native proof confirms every retired native rule; custom proof is per rule"
        );
        assert_eq!(pending[0].id, config.cleanup_rules[1].rule_id);
        assert!(
            !db.repo
                .port_forwarding_config_for_client(client)
                .await
                .unwrap()
                .native_cleanup_pending
        );
        snapshot.removed_rules = config.cleanup_rules;
        snapshot.observed_unix += 1;
        db.repo
            .record_port_forward_runtime_snapshot(client, &snapshot)
            .await
            .unwrap();
        // A manual list read must reflect persisted confirmation, independent of UI updates.
        assert!(db
            .repo
            .list_port_forward_rules()
            .await
            .unwrap()
            .iter()
            .all(|rule| rule.client_id != client || rule.deleted_at.is_none()));
        assert!(db
            .repo
            .port_forwarding_config_for_client(client)
            .await
            .unwrap()
            .cleanup_rules
            .is_empty());
    }
    let remaining = db.repo.list_port_forward_rules().await.unwrap();
    assert_eq!(
        remaining.iter().map(|rule| rule.id).collect::<Vec<_>>(),
        survivors
    );
    db.cleanup().await;
}
