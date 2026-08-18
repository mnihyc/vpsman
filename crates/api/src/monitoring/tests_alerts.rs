use super::*;
use serde_json::json;
use std::collections::BTreeMap;
use vpsman_common::AgentCapabilitySnapshot;

use crate::model_alert_states::{BulkFleetAlertStateItem, BulkUpdateFleetAlertStatesRequest};

#[tokio::test]
async fn memory_policy_lifecycle_is_explicitly_postgres_only() {
    let repo = Repository::Memory(MemoryState::default());
    assert_eq!(repo.evaluate_policy_rules().await.unwrap(), 0);
    repo.reconcile_operational_alerts().await.unwrap();
    assert!(repo
        .list_policy_alerts(&PolicyAlertQuery {
            limit: Some(20),
            client_id: None,
            severity: None,
            category: None,
            policy_group_id: None,
        })
        .await
        .unwrap()
        .is_empty());
    if let Repository::Memory(memory) = &repo {
        assert!(memory.policy_alerts.read().await.is_empty());
        assert!(memory
            .webhook_events
            .read()
            .await
            .iter()
            .all(|event| { event.kind != "alert.triggered" && event.kind != "alert.resolved" }));
    }
}

#[tokio::test]
async fn policy_live_enrichment_requires_rule_capability_only_for_rule_selectors() {
    let ordinary_repo = Repository::Memory(MemoryState::default());
    let operator = test_operator();
    let ordinary = ordinary_repo
        .upsert_fleet_alert_policy(
            &CreateFleetAlertPolicyRequest {
                id: None,
                name: "ordinary-policy".to_string(),
                enabled: true,
                selector_expression: "status:online".to_string(),
                rules: vec![metric_policy_rule_request(None, "load", "warning")],
                notes: None,
                confirmed: true,
                preview_hash: None,
            },
            &operator,
        )
        .await
        .unwrap();
    assert!(ordinary_repo
        .list_fleet_alert_policies(20, None, None, None, false)
        .await
        .is_ok());
    assert!(ordinary_repo
        .get_fleet_alert_policy(ordinary.id, false)
        .await
        .is_ok());

    let rule_repo = Repository::Memory(MemoryState::default());
    let rule_policy = rule_repo
        .upsert_fleet_alert_policy(
            &CreateFleetAlertPolicyRequest {
                id: None,
                name: "rule-policy".to_string(),
                enabled: true,
                selector_expression: "vps.rules:traffic.reset_day >= 15".to_string(),
                rules: vec![metric_policy_rule_request(None, "load", "warning")],
                notes: None,
                confirmed: true,
                preview_hash: None,
            },
            &operator,
        )
        .await
        .unwrap();
    let list_error = rule_repo
        .list_fleet_alert_policies(20, None, None, None, false)
        .await
        .unwrap_err();
    assert!(list_error
        .to_string()
        .contains("vps_rule_selector_scope_required"));
    let get_error = rule_repo
        .get_fleet_alert_policy(rule_policy.id, false)
        .await
        .unwrap_err();
    assert!(get_error
        .to_string()
        .contains("vps_rule_selector_scope_required"));
}

#[tokio::test]
async fn filter_limit_regression_internal_traffic_accounting_is_unbounded() {
    let memory = MemoryState::default();
    memory.agents.write().await.push(AgentView {
        id: "zzz-rule-target".to_string(),
        display_name: "Rule Target".to_string(),
        status: "online".to_string(),
        tags: Vec::new(),
        registration_ip: None,
        last_ip: None,
        last_seen_at: None,
        arch: None,
        internal_build_number: 1,
        process_incarnation_id: None,
        stale_since: None,
        stale_reason: None,
        capabilities: AgentCapabilitySnapshot::default(),
    });
    let stored_rule =
        |client_id: String, key: &str, value_raw: &str, value_json: serde_json::Value| {
            crate::model_alert_policies::VpsRuleValueRecord {
                client_id,
                key: key.to_string(),
                value_raw: value_raw.to_string(),
                stored_value_raw: None,
                value_json,
                parsed_display: value_raw.to_string(),
                state: "ok".to_string(),
                validation_errors: Vec::new(),
                source_kind: "operator".to_string(),
                source_id: None,
                updated_by: None,
                updated_at: "1".to_string(),
            }
        };
    let mut rules = (0..5000)
        .map(|index| {
            stored_rule(
                format!("aaa-filler-{index:04}"),
                "traffic.reset_day",
                "1",
                json!({"day": 1}),
            )
        })
        .collect::<Vec<_>>();
    rules.extend([
        stored_rule(
            "zzz-rule-target".to_string(),
            "traffic.selectors",
            "eth0",
            json!({
                "selectors": [{
                    "source": "host",
                    "interface": "eth0",
                    "direction": "total",
                    "canonical": "eth0"
                }]
            }),
        ),
        stored_rule(
            "zzz-rule-target".to_string(),
            "traffic.reset_day",
            "7",
            json!({"day": 7}),
        ),
        stored_rule(
            "zzz-rule-target".to_string(),
            "traffic.quota.total",
            "1GB",
            json!({"bytes": 1_000_000_000_i64}),
        ),
    ]);
    *memory.vps_rule_values.write().await = rules;
    let repo = Repository::Memory(memory);

    let accounting = repo
        .get_traffic_accounting("zzz-rule-target")
        .await
        .unwrap();
    assert_eq!(accounting.reset_day, Some(7));
    assert_eq!(accounting.quota_total_bytes, Some(1_000_000_000));
    assert_eq!(accounting.selectors, vec!["eth0"]);

    let public_rows = repo
        .list_vps_rules(&VpsRuleQuery {
            limit: Some(2),
            client_id: Some("zzz-rule-target".to_string()),
            selector_expression: None,
            key: None,
            state: None,
        })
        .await
        .unwrap();
    assert_eq!(public_rows.len(), 2);
    assert_eq!(public_rows[0].key, "traffic.quota.total");
    assert_eq!(public_rows[1].key, "traffic.reset_day");
}

#[tokio::test]
async fn vps_rules_dry_run_returns_invalid_billing_row_without_persisting() {
    use tower::ServiceExt as _;

    let repo = Repository::Memory(MemoryState::default());
    if let Repository::Memory(memory) = &repo {
        memory.agents.write().await.push(AgentView {
            id: "v-1".to_string(),
            display_name: "VPS 1".to_string(),
            status: "online".to_string(),
            tags: Vec::new(),
            registration_ip: None,
            last_ip: None,
            last_seen_at: None,
            arch: None,
            internal_build_number: 1,
            process_incarnation_id: None,
            stale_since: None,
            stale_reason: None,
            capabilities: AgentCapabilitySnapshot::default(),
        });
    }
    let auth = repo
        .bootstrap_operator(&BootstrapOperatorRequest {
            username: "admin".to_string(),
            password: "admin-password-123".to_string(),
        })
        .await
        .unwrap();
    let state = alert_test_state(repo.clone());
    let response = crate::routes::build_router(state)
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/api/v1/vps-rules/dry-run")
                .header(
                    axum::http::header::AUTHORIZATION,
                    format!("Bearer {}", auth.access_token),
                )
                .header(axum::http::header::CONTENT_TYPE, "application/json")
                .body(axum::body::Body::from(
                    serde_json::to_vec(&json!({
                        "operation": "upsert",
                        "selector_expression": "id:v-1",
                        "values": {"billing.price": "10 USD/week"},
                        "keys": []
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let preview: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(preview["matched_vps_count"], 1);
    assert_eq!(preview["changed_row_count"], 0);
    assert_eq!(preview["invalid_row_count"], 1);
    assert_eq!(preview["changes"][0]["action"], "invalid");
    assert_eq!(
        preview["changes"][0]["validation_errors"],
        json!(["billing_plan_period_invalid"])
    );
    assert!(repo
        .list_vps_rules(&VpsRuleQuery {
            limit: None,
            client_id: Some("v-1".to_string()),
            selector_expression: None,
            key: None,
            state: None,
        })
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn vps_rule_spacing_variants_are_canonical_and_confirmed_edit_is_a_row_noop() {
    let repo = Repository::Memory(MemoryState::default());
    let Repository::Memory(memory) = &repo else {
        unreachable!();
    };
    memory.agents.write().await.push(AgentView {
        id: "v-1".to_string(),
        display_name: "VPS 1".to_string(),
        status: "online".to_string(),
        tags: Vec::new(),
        registration_ip: None,
        last_ip: None,
        last_seen_at: None,
        arch: None,
        internal_build_number: 1,
        process_incarnation_id: None,
        stale_since: None,
        stale_reason: None,
        capabilities: AgentCapabilitySnapshot::default(),
    });
    let operator = test_operator();
    let first_values = BTreeMap::from([(
        format!(" {VPS_RULE_KEY_NETWORK_PORT_SPEED} "),
        " 001.500   gbps ".to_string(),
    )]);
    let first_preview = repo
        .dry_run_vps_rules(&VpsRulesDryRunRequest {
            operation: "upsert".to_string(),
            selector_expression: "id:v-1".to_string(),
            values: first_values.clone(),
            keys: Vec::new(),
        })
        .await
        .unwrap();
    assert_eq!(first_preview.changed_row_count, 1);
    assert_eq!(first_preview.changes[0].after.as_deref(), Some("1.5 Gbps"));
    repo.bulk_upsert_vps_rules(
        &VpsRulesBulkUpsertRequest {
            selector_expression: "id:v-1".to_string(),
            values: first_values,
            confirmed: true,
            preview_hash: first_preview.preview_hash,
        },
        &operator,
    )
    .await
    .unwrap();
    {
        let mut rows = memory.vps_rule_values.write().await;
        assert_eq!(rows[0].key, VPS_RULE_KEY_NETWORK_PORT_SPEED);
        // Simulate a row written before canonical save-time normalization.
        rows[0].value_raw = " 001.500   gbps ".to_string();
        rows[0].stored_value_raw = None;
        rows[0].updated_at = "legacy-timestamp".to_string();
    }

    let edit_values = BTreeMap::from([(
        VPS_RULE_KEY_NETWORK_PORT_SPEED.to_string(),
        "1.5Gbps".to_string(),
    )]);
    let normalization_preview = repo
        .dry_run_vps_rules(&VpsRulesDryRunRequest {
            operation: "upsert".to_string(),
            selector_expression: "id:v-1".to_string(),
            values: edit_values.clone(),
            keys: Vec::new(),
        })
        .await
        .unwrap();
    assert_eq!(normalization_preview.changed_row_count, 1);
    assert_eq!(normalization_preview.changes[0].action, "set");
    assert_eq!(
        normalization_preview.changes[0].before.as_deref(),
        Some(" 001.500   gbps ")
    );
    assert_eq!(
        normalization_preview.changes[0].after.as_deref(),
        Some("1.5 Gbps")
    );
    let audit_count_before_normalization = memory.audits.read().await.len();
    repo.bulk_upsert_vps_rules(
        &VpsRulesBulkUpsertRequest {
            selector_expression: "id:v-1".to_string(),
            values: edit_values.clone(),
            confirmed: true,
            preview_hash: normalization_preview.preview_hash,
        },
        &operator,
    )
    .await
    .unwrap();
    let before_noop = memory.vps_rule_values.read().await[0].clone();
    assert_eq!(before_noop.value_raw, "1.5 Gbps");
    assert_eq!(before_noop.updated_by, Some(operator.operator.id));
    assert_eq!(
        memory.audits.read().await.len(),
        audit_count_before_normalization + 1
    );
    let before_noop_audit_count = memory.audits.read().await.len();

    let edit_preview = repo
        .dry_run_vps_rules(&VpsRulesDryRunRequest {
            operation: "upsert".to_string(),
            selector_expression: "id:v-1".to_string(),
            values: edit_values.clone(),
            keys: Vec::new(),
        })
        .await
        .unwrap();
    assert_eq!(edit_preview.changed_row_count, 0);
    assert_eq!(edit_preview.changes[0].action, "unchanged");
    repo.bulk_upsert_vps_rules(
        &VpsRulesBulkUpsertRequest {
            selector_expression: "id:v-1".to_string(),
            values: edit_values,
            confirmed: true,
            preview_hash: edit_preview.preview_hash,
        },
        &operator,
    )
    .await
    .unwrap();
    let after = memory.vps_rule_values.read().await[0].clone();
    assert_eq!(after.value_raw, "1.5 Gbps");
    assert_eq!(
        after.value_json,
        json!({"bps": 1_500_000_000_i64, "display": "1.5 Gbps"})
    );
    assert_eq!(after.parsed_display, "1.5 Gbps");
    assert_eq!(after.updated_at, before_noop.updated_at);
    assert_eq!(after.updated_by, before_noop.updated_by);
    assert_eq!(memory.audits.read().await.len(), before_noop_audit_count);

    let duplicate_error = repo
        .dry_run_vps_rules(&VpsRulesDryRunRequest {
            operation: "upsert".to_string(),
            selector_expression: "id:v-1".to_string(),
            values: BTreeMap::from([
                (
                    VPS_RULE_KEY_NETWORK_PORT_SPEED.to_string(),
                    "1 Gbps".to_string(),
                ),
                (
                    format!(" {VPS_RULE_KEY_NETWORK_PORT_SPEED} "),
                    "2 Gbps".to_string(),
                ),
            ]),
            keys: Vec::new(),
        })
        .await
        .unwrap_err();
    assert!(duplicate_error
        .to_string()
        .contains("vps_rules_duplicate_key"));

    let unset_keys = vec![format!(" {VPS_RULE_KEY_NETWORK_PORT_SPEED} ")];
    let unset_preview = repo
        .dry_run_vps_rules(&VpsRulesDryRunRequest {
            operation: "unset".to_string(),
            selector_expression: "id:v-1".to_string(),
            values: BTreeMap::new(),
            keys: unset_keys.clone(),
        })
        .await
        .unwrap();
    assert_eq!(unset_preview.changed_row_count, 1);
    repo.bulk_unset_vps_rules(
        &VpsRulesBulkUnsetRequest {
            selector_expression: "id:v-1".to_string(),
            keys: unset_keys,
            confirmed: true,
            preview_hash: unset_preview.preview_hash,
        },
        &operator,
    )
    .await
    .unwrap();
    assert!(memory.vps_rule_values.read().await.is_empty());
}

#[tokio::test]
async fn product_name_save_and_equivalent_whitespace_edit_are_canonical_and_noop() {
    let repo = Repository::Memory(MemoryState::default());
    let Repository::Memory(memory) = &repo else {
        unreachable!();
    };
    memory.agents.write().await.push(AgentView {
        id: "product-vps".to_string(),
        display_name: "Product VPS".to_string(),
        status: "online".to_string(),
        tags: Vec::new(),
        registration_ip: None,
        last_ip: None,
        last_seen_at: None,
        arch: None,
        internal_build_number: 1,
        process_incarnation_id: None,
        stale_since: None,
        stale_reason: None,
        capabilities: AgentCapabilitySnapshot::default(),
    });
    let operator = test_operator();
    let key = vpsman_common::VPS_RULE_KEY_PRODUCT_NAME;
    let values = BTreeMap::from([(key.to_string(), "  Storage-Box\t 4  ".to_string())]);
    let preview = repo
        .dry_run_vps_rules(&VpsRulesDryRunRequest {
            operation: "upsert".to_string(),
            selector_expression: "id:product-vps".to_string(),
            values: values.clone(),
            keys: Vec::new(),
        })
        .await
        .unwrap();
    assert_eq!(preview.changed_row_count, 1);
    assert_eq!(preview.changes[0].after.as_deref(), Some("Storage-Box 4"));
    repo.bulk_upsert_vps_rules(
        &VpsRulesBulkUpsertRequest {
            selector_expression: "id:product-vps".to_string(),
            values,
            confirmed: true,
            preview_hash: preview.preview_hash,
        },
        &operator,
    )
    .await
    .unwrap();
    let before = memory.vps_rule_values.read().await[0].clone();
    let audit_count = memory.audits.read().await.len();

    let equivalent_values =
        BTreeMap::from([(key.to_string(), "\n Storage-Box     4 \t".to_string())]);
    let equivalent_preview = repo
        .dry_run_vps_rules(&VpsRulesDryRunRequest {
            operation: "upsert".to_string(),
            selector_expression: "id:product-vps".to_string(),
            values: equivalent_values.clone(),
            keys: Vec::new(),
        })
        .await
        .unwrap();
    assert_eq!(equivalent_preview.changed_row_count, 0);
    assert_eq!(equivalent_preview.changes[0].action, "unchanged");
    repo.bulk_upsert_vps_rules(
        &VpsRulesBulkUpsertRequest {
            selector_expression: "id:product-vps".to_string(),
            values: equivalent_values,
            confirmed: true,
            preview_hash: equivalent_preview.preview_hash,
        },
        &operator,
    )
    .await
    .unwrap();

    let after = memory.vps_rule_values.read().await[0].clone();
    assert_eq!(after.value_raw, "Storage-Box 4");
    assert_eq!(
        after.value_json,
        json!({"name": "Storage-Box 4", "display": "Storage-Box 4"})
    );
    assert_eq!(after.updated_at, before.updated_at);
    assert_eq!(after.updated_by, before.updated_by);
    assert_eq!(memory.audits.read().await.len(), audit_count);
}

#[tokio::test]
async fn concurrent_confirmations_reject_the_stale_self_referential_rule_preview() {
    let repo = Repository::Memory(MemoryState::default());
    let Repository::Memory(memory) = &repo else {
        unreachable!();
    };
    memory.agents.write().await.push(AgentView {
        id: "v-1".to_string(),
        display_name: "VPS 1".to_string(),
        status: "online".to_string(),
        tags: Vec::new(),
        registration_ip: None,
        last_ip: None,
        last_seen_at: None,
        arch: None,
        internal_build_number: 1,
        process_incarnation_id: None,
        stale_since: None,
        stale_reason: None,
        capabilities: AgentCapabilitySnapshot::default(),
    });
    let parsed = vpsman_common::parse_vps_rule_value(VPS_RULE_KEY_TRAFFIC_RESET_DAY, "1").unwrap();
    memory
        .vps_rule_values
        .write()
        .await
        .push(crate::model_alert_policies::VpsRuleValueRecord {
            client_id: "v-1".to_string(),
            key: VPS_RULE_KEY_TRAFFIC_RESET_DAY.to_string(),
            value_raw: parsed.raw,
            stored_value_raw: None,
            value_json: parsed.json,
            parsed_display: parsed.display,
            state: "ok".to_string(),
            validation_errors: Vec::new(),
            source_kind: "operator".to_string(),
            source_id: None,
            updated_by: None,
            updated_at: "1".to_string(),
        });
    let selector_expression = "vps.rules:traffic.reset_day <= 1".to_string();
    let values = BTreeMap::from([(VPS_RULE_KEY_TRAFFIC_RESET_DAY.to_string(), "2".to_string())]);
    let preview = repo
        .dry_run_vps_rules(&VpsRulesDryRunRequest {
            operation: "upsert".to_string(),
            selector_expression: selector_expression.clone(),
            values: values.clone(),
            keys: Vec::new(),
        })
        .await
        .unwrap();
    assert_eq!(preview.matched_vps_count, 1);
    let left_request = VpsRulesBulkUpsertRequest {
        selector_expression: selector_expression.clone(),
        values: values.clone(),
        confirmed: true,
        preview_hash: preview.preview_hash.clone(),
    };
    let right_request = VpsRulesBulkUpsertRequest {
        selector_expression,
        values,
        confirmed: true,
        preview_hash: preview.preview_hash,
    };
    let operator = test_operator();
    let (left, right) = tokio::join!(
        repo.bulk_upsert_vps_rules(&left_request, &operator),
        repo.bulk_upsert_vps_rules(&right_request, &operator),
    );
    let results = [left, right];
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    let error = results
        .into_iter()
        .find_map(Result::err)
        .expect("one stale confirmation");
    assert!(error
        .to_string()
        .contains("vps_rules_preview_hash_mismatch"));
    let rows = memory.vps_rule_values.read().await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].value_raw, "2");
    assert_eq!(memory.audits.read().await.len(), 1);
}

#[tokio::test]
async fn configured_traffic_without_a_quota_remains_healthy() {
    let memory = MemoryState::default();
    let now = chrono::Utc::now().timestamp();
    memory.agents.write().await.push(AgentView {
        id: "v-1".to_string(),
        display_name: "VPS 1".to_string(),
        status: "online".to_string(),
        tags: Vec::new(),
        registration_ip: None,
        last_ip: None,
        last_seen_at: Some(now.to_string()),
        arch: None,
        internal_build_number: 1,
        process_incarnation_id: None,
        stale_since: None,
        stale_reason: None,
        capabilities: AgentCapabilitySnapshot::default(),
    });
    let stored_rule = |key: &str, value_raw: &str, value_json: serde_json::Value| {
        crate::model_alert_policies::VpsRuleValueRecord {
            client_id: "v-1".to_string(),
            key: key.to_string(),
            value_raw: value_raw.to_string(),
            stored_value_raw: None,
            value_json,
            parsed_display: value_raw.to_string(),
            state: "ok".to_string(),
            validation_errors: Vec::new(),
            source_kind: "test".to_string(),
            source_id: None,
            updated_by: None,
            updated_at: now.to_string(),
        }
    };
    memory.vps_rule_values.write().await.extend([
        stored_rule(VPS_RULE_KEY_TRAFFIC_RESET_DAY, "1", json!({"day": 1})),
        stored_rule(
            VPS_RULE_KEY_TRAFFIC_SELECTORS,
            "eth0",
            json!({
                "selectors": [{
                    "source": "host",
                    "interface": "eth0",
                    "direction": "total",
                    "canonical": "eth0"
                }]
            }),
        ),
    ]);
    memory.traffic_counter_samples.write().await.extend([
        TrafficCounterSampleRecord {
            client_id: "v-1".to_string(),
            source_kind: "host".to_string(),
            interface: "eth0".to_string(),
            observed_at: (now - 60).to_string(),
            observed_unix: now - 60,
            rx_bytes: 1_000,
            tx_bytes: 2_000,
            rx_counter_epoch: 0,
            tx_counter_epoch: 0,
            sample_source: "test".to_string(),
        },
        TrafficCounterSampleRecord {
            client_id: "v-1".to_string(),
            source_kind: "host".to_string(),
            interface: "eth0".to_string(),
            observed_at: now.to_string(),
            observed_unix: now,
            rx_bytes: 1_100,
            tx_bytes: 2_200,
            rx_counter_epoch: 0,
            tx_counter_epoch: 0,
            sample_source: "test".to_string(),
        },
    ]);
    let accounting = Repository::Memory(memory)
        .get_traffic_accounting("v-1")
        .await
        .unwrap();

    assert_eq!(accounting.state, "ok");
    assert!(accounting.incomplete_reasons.is_empty());
    assert_eq!(accounting.quota_total_bytes, None);
    assert_eq!(accounting.cycle_percent, None);
    assert_eq!(accounting.rx_bytes, 100);
    assert_eq!(accounting.tx_bytes, 200);
}

#[tokio::test]
async fn fleet_alert_state_bulk_is_revisioned_atomic_and_batch_audited() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = test_operator();
    let initial = BulkUpdateFleetAlertStatesRequest {
        action: "acknowledge".to_string(),
        items: vec![
            BulkFleetAlertStateItem {
                alert_id: "alert:bulk-b".to_string(),
                expected_revision: 0,
            },
            BulkFleetAlertStateItem {
                alert_id: "alert:bulk-a".to_string(),
                expected_revision: 0,
            },
        ],
        muted_for_secs: None,
        reason: Some("reviewed together".to_string()),
        confirmed: true,
    };
    let response = repo
        .bulk_update_fleet_alert_states(&initial, &operator)
        .await
        .unwrap();
    assert_eq!(
        response
            .states
            .iter()
            .map(|state| state.alert_id.as_str())
            .collect::<Vec<_>>(),
        ["alert:bulk-a", "alert:bulk-b"]
    );
    assert!(response.states.iter().all(|state| state.revision == 1));

    let Repository::Memory(memory) = &repo else {
        unreachable!()
    };
    let audits = memory.audits.read().await;
    assert_eq!(audits.len(), 2);
    assert!(audits.iter().all(|audit| {
        audit.metadata["batch_id"] == json!(response.batch_id)
            && audit.metadata["batch_size"] == 2
            && audit.metadata["batch_action"] == "acknowledge"
    }));
    drop(audits);

    let stale = BulkUpdateFleetAlertStatesRequest {
        action: "escalate".to_string(),
        items: vec![
            BulkFleetAlertStateItem {
                alert_id: "alert:bulk-a".to_string(),
                expected_revision: 1,
            },
            BulkFleetAlertStateItem {
                alert_id: "alert:bulk-new".to_string(),
                expected_revision: 1,
            },
        ],
        muted_for_secs: None,
        reason: Some("must remain atomic".to_string()),
        confirmed: true,
    };
    let error = repo
        .bulk_update_fleet_alert_states(&stale, &operator)
        .await
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("fleet_alert_state_snapshot_stale"));
    let states = memory.fleet_alert_states.read().await;
    assert_eq!(states.len(), 2);
    assert!(states.iter().all(|state| {
        state.state == "acknowledged" && state.revision == 1 && state.escalation_level == 0
    }));
    drop(states);
    assert_eq!(memory.audits.read().await.len(), 2);

    let escalated = repo
        .bulk_update_fleet_alert_states(
            &BulkUpdateFleetAlertStatesRequest {
                action: "escalate".to_string(),
                items: vec![BulkFleetAlertStateItem {
                    alert_id: "alert:bulk-a".to_string(),
                    expected_revision: 1,
                }],
                muted_for_secs: None,
                reason: None,
                confirmed: true,
            },
            &operator,
        )
        .await
        .unwrap();
    assert_eq!(escalated.states[0].escalation_level, 1);
    assert_eq!(escalated.states[0].revision, 2);
    let escalated_again = repo
        .bulk_update_fleet_alert_states(
            &BulkUpdateFleetAlertStatesRequest {
                action: "escalate".to_string(),
                items: vec![BulkFleetAlertStateItem {
                    alert_id: "alert:bulk-a".to_string(),
                    expected_revision: 2,
                }],
                muted_for_secs: None,
                reason: None,
                confirmed: true,
            },
            &operator,
        )
        .await
        .unwrap();
    assert_eq!(escalated_again.states[0].escalation_level, 2);
    assert_eq!(escalated_again.states[0].revision, 3);
}

#[tokio::test]
async fn fleet_alert_notification_dispatch_rejects_channel_overflow_explicitly() {
    let repo = Repository::Memory(MemoryState::default());
    if let Repository::Memory(memory) = &repo {
        let mut channels = memory.fleet_alert_notification_channels.write().await;
        for index in 0..=1_000 {
            channels.push(
                crate::model_alert_notifications::FleetAlertNotificationChannelView {
                    id: Uuid::new_v4(),
                    name: format!("channel-{index:04}"),
                    scope_kind: "global".to_string(),
                    scope_value: None,
                    min_severity: "warning".to_string(),
                    categories: Vec::new(),
                    operator_states: Vec::new(),
                    delivery_kind: "webhook".to_string(),
                    target: "https://hooks.acme.com/fleet".to_string(),
                    cooldown_secs: 60,
                    enabled: true,
                    configuration_error: None,
                    notes: None,
                    actor_id: None,
                    created_at: "0".to_string(),
                    updated_at: "0".to_string(),
                },
            );
        }
    }

    let state = alert_test_state(repo);
    let error = state
        .dispatch_fleet_alert_notifications(
            &FleetAlertNotificationDispatchRequest {
                limit: Some(1),
                client_id: None,
                severity: None,
                category: None,
                operator_state: None,
                include_muted: None,
                dry_run: Some(true),
                preview_hash: None,
                confirmed: false,
            },
            &test_operator(),
        )
        .await
        .unwrap_err()
        .to_string();

    assert!(error.contains("fleet_alert_notification_dispatch_channel_limit_exceeded"));
}

#[tokio::test]
async fn disabled_alert_notification_channel_cancels_retryable_deliveries() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = test_operator();
    let channel_id = Uuid::new_v4();
    repo.upsert_fleet_alert_notification_channel(
        &CreateFleetAlertNotificationChannelRequest {
            id: Some(channel_id),
            name: "edge-webhook".to_string(),
            scope_kind: "tag".to_string(),
            scope_value: Some("edge".to_string()),
            min_severity: Some("warning".to_string()),
            categories: Some(vec!["agent_status".to_string()]),
            operator_states: Some(vec!["open".to_string()]),
            delivery_kind: "webhook".to_string(),
            target: "https://hooks.acme.com/fleet".to_string(),
            cooldown_secs: Some(900),
            enabled: Some(true),
            notes: None,
            confirmed: true,
        },
        &operator,
    )
    .await
    .unwrap();
    let deliveries = repo
        .record_fleet_alert_notification_deliveries(
            &[FleetAlertNotificationCandidate {
                channel_id,
                channel_name: "edge-webhook".to_string(),
                alert_id: "agent_status:agent:edge-a".to_string(),
                alert_severity: "critical".to_string(),
                alert_category: "agent_status".to_string(),
                status: "queued".to_string(),
                delivery_kind: "webhook".to_string(),
                target: "https://hooks.acme.com/fleet".to_string(),
                dedupe_key: "fleet-alert-notification:test".to_string(),
                payload: json!({"schema": "test"}),
                cooldown_until_unix: 0,
            }],
            &operator,
        )
        .await
        .unwrap();
    assert_eq!(deliveries.len(), 1);

    repo.upsert_fleet_alert_notification_channel(
        &CreateFleetAlertNotificationChannelRequest {
            id: Some(channel_id),
            name: "edge-webhook".to_string(),
            scope_kind: "tag".to_string(),
            scope_value: Some("edge".to_string()),
            min_severity: Some("warning".to_string()),
            categories: Some(vec!["agent_status".to_string()]),
            operator_states: Some(vec!["open".to_string()]),
            delivery_kind: "webhook".to_string(),
            target: "https://hooks.acme.com/fleet".to_string(),
            cooldown_secs: Some(900),
            enabled: Some(false),
            notes: None,
            confirmed: true,
        },
        &operator,
    )
    .await
    .unwrap();

    let canceled = repo
        .list_fleet_alert_notification_deliveries(20, None, None, Some("canceled_disabled"))
        .await
        .unwrap();
    assert_eq!(canceled.len(), 1);
    assert_eq!(canceled[0].id, deliveries[0].id);
    assert!(canceled[0]
        .error
        .as_deref()
        .is_some_and(|error| error.contains("disabled")));
    let claimed = repo
        .claim_fleet_alert_notification_deliveries_for_process(
            &[deliveries[0].id],
            Uuid::new_v4(),
            60,
        )
        .await
        .unwrap();
    assert!(claimed.is_empty());
}

#[tokio::test]
async fn deleted_alert_notification_channel_preserves_and_cancels_delivery_history() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = test_operator();
    let channel_id = Uuid::new_v4();
    repo.upsert_fleet_alert_notification_channel(
        &CreateFleetAlertNotificationChannelRequest {
            id: Some(channel_id),
            name: "deleted-edge-webhook".to_string(),
            scope_kind: "global".to_string(),
            scope_value: None,
            min_severity: Some("warning".to_string()),
            categories: Some(vec!["agent_status".to_string()]),
            operator_states: Some(vec!["open".to_string()]),
            delivery_kind: "webhook".to_string(),
            target: "https://hooks.acme.com/fleet".to_string(),
            cooldown_secs: Some(900),
            enabled: Some(true),
            notes: None,
            confirmed: true,
        },
        &operator,
    )
    .await
    .unwrap();
    let created = repo
        .record_fleet_alert_notification_deliveries(
            &[FleetAlertNotificationCandidate {
                channel_id,
                channel_name: "deleted-edge-webhook".to_string(),
                alert_id: "agent_status:agent:edge-a".to_string(),
                alert_severity: "critical".to_string(),
                alert_category: "agent_status".to_string(),
                status: "queued".to_string(),
                delivery_kind: "webhook".to_string(),
                target: "https://hooks.acme.com/fleet".to_string(),
                dedupe_key: "fleet-alert-notification:deleted-test".to_string(),
                payload: json!({"schema": "test"}),
                cooldown_until_unix: 0,
            }],
            &operator,
        )
        .await
        .unwrap();

    let error = repo
        .delete_fleet_alert_notification_channel(channel_id, "stale-name", &operator)
        .await
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("fleet_alert_notification_channel_delete_review_stale"));
    assert_eq!(
        repo.list_fleet_alert_notification_channels(20, None, None, None, None)
            .await
            .unwrap()
            .len(),
        1
    );

    repo.delete_fleet_alert_notification_channel(channel_id, "deleted-edge-webhook", &operator)
        .await
        .unwrap();

    let stale_dispatch = repo
        .record_fleet_alert_notification_deliveries(
            &[FleetAlertNotificationCandidate {
                channel_id,
                channel_name: "deleted-edge-webhook".to_string(),
                alert_id: "agent_status:agent:edge-b".to_string(),
                alert_severity: "critical".to_string(),
                alert_category: "agent_status".to_string(),
                status: "queued".to_string(),
                delivery_kind: "webhook".to_string(),
                target: "https://hooks.acme.com/fleet".to_string(),
                dedupe_key: "fleet-alert-notification:stale-deleted-test".to_string(),
                payload: json!({"schema": "test"}),
                cooldown_until_unix: 0,
            }],
            &operator,
        )
        .await
        .unwrap();
    assert!(stale_dispatch.is_empty());

    assert!(repo
        .list_fleet_alert_notification_channels(20, None, None, None, None)
        .await
        .unwrap()
        .is_empty());
    let retained = repo
        .list_fleet_alert_notification_deliveries(20, Some(channel_id), None, None)
        .await
        .unwrap();
    assert_eq!(retained.len(), 1);
    assert_eq!(retained[0].id, created[0].id);
    assert_eq!(retained[0].status, "canceled_disabled");
    assert_eq!(
        retained[0].error.as_deref(),
        Some("fleet alert notification channel deleted")
    );
    assert!(
        repo.claim_fleet_alert_notification_deliveries_for_process(
            &[created[0].id],
            Uuid::new_v4(),
            60,
        )
        .await
        .unwrap()
        .is_empty()
    );
}

#[tokio::test]
async fn disabled_webhook_rule_cancels_retryable_deliveries() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = test_operator();
    let rule_id = Uuid::new_v4();
    repo.upsert_webhook_rule(
        &crate::model_webhook_rules::CreateWebhookRuleRequest {
            id: Some(rule_id),
            name: "edge-rule".to_string(),
            enabled: true,
            expression: "status = stale".to_string(),
            target: "https://hooks.acme.com/webhook".to_string(),
            body_template: String::new(),
            signing_secret: None,
            clear_signing_secret: false,
            cooldown_secs: Some(60),
            notes: None,
            confirmed: true,
        },
        &operator,
    )
    .await
    .unwrap();
    let deliveries = repo
        .record_webhook_rule_deliveries(&[
            crate::model_webhook_rules::WebhookRuleDeliveryCandidate {
                rule_id,
                rule_name: "edge-rule".to_string(),
                event_kind: "manual.test".to_string(),
                event_id: "event-1".to_string(),
                target: "https://hooks.acme.com/webhook".to_string(),
                dedupe_key: "webhook-rule:test".to_string(),
                payload: json!({"schema": "test"}),
                matched_vps: Vec::new(),
                message: "test".to_string(),
                rule_revision_hash: "test-revision".to_string(),
                signing_secret: None,
                cooldown_until_unix: 0,
                actor_id: Some(operator.operator.id),
            },
        ])
        .await
        .unwrap();
    assert_eq!(deliveries.len(), 1);

    repo.upsert_webhook_rule(
        &crate::model_webhook_rules::CreateWebhookRuleRequest {
            id: Some(rule_id),
            name: "edge-rule".to_string(),
            enabled: false,
            expression: "status = stale".to_string(),
            target: "https://hooks.acme.com/webhook".to_string(),
            body_template: String::new(),
            signing_secret: None,
            clear_signing_secret: false,
            cooldown_secs: Some(60),
            notes: None,
            confirmed: true,
        },
        &operator,
    )
    .await
    .unwrap();

    let canceled = repo
        .list_webhook_rule_deliveries(20, Some(rule_id), None, Some("canceled_disabled"))
        .await
        .unwrap();
    assert_eq!(canceled.len(), 1);
    assert_eq!(canceled[0].id, deliveries[0].id);
    assert!(canceled[0]
        .error
        .as_deref()
        .is_some_and(|error| error.contains("disabled")));
    let claimed = repo
        .claim_webhook_rule_deliveries_for_process(&[deliveries[0].id], Uuid::new_v4(), 60)
        .await
        .unwrap();
    assert!(claimed.is_empty());
}

#[tokio::test]
async fn deleted_webhook_rule_preserves_and_cancels_delivery_history() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = test_operator();
    let rule_id = Uuid::new_v4();
    repo.upsert_webhook_rule(
        &crate::model_webhook_rules::CreateWebhookRuleRequest {
            id: Some(rule_id),
            name: "deleted-edge-rule".to_string(),
            enabled: true,
            expression: "status = stale".to_string(),
            target: "https://hooks.acme.com/webhook".to_string(),
            body_template: String::new(),
            signing_secret: None,
            clear_signing_secret: false,
            cooldown_secs: Some(60),
            notes: None,
            confirmed: true,
        },
        &operator,
    )
    .await
    .unwrap();
    let created = repo
        .record_webhook_rule_deliveries(&[
            crate::model_webhook_rules::WebhookRuleDeliveryCandidate {
                rule_id,
                rule_name: "deleted-edge-rule".to_string(),
                event_kind: "manual.test".to_string(),
                event_id: "deleted-event-1".to_string(),
                target: "https://hooks.acme.com/webhook".to_string(),
                dedupe_key: "webhook-rule:deleted-test".to_string(),
                payload: json!({"schema": "test"}),
                matched_vps: Vec::new(),
                message: "test".to_string(),
                rule_revision_hash: "deleted-test-revision".to_string(),
                signing_secret: None,
                cooldown_until_unix: 0,
                actor_id: Some(operator.operator.id),
            },
        ])
        .await
        .unwrap();

    let error = repo
        .delete_webhook_rule(rule_id, "stale-name", &operator)
        .await
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("webhook_rule_delete_review_stale"));
    assert_eq!(repo.list_webhook_rules(20, None).await.unwrap().len(), 1);

    repo.delete_webhook_rule(rule_id, "deleted-edge-rule", &operator)
        .await
        .unwrap();

    let stale_dispatch = repo
        .record_webhook_rule_deliveries(&[
            crate::model_webhook_rules::WebhookRuleDeliveryCandidate {
                rule_id,
                rule_name: "deleted-edge-rule".to_string(),
                event_kind: "manual.test".to_string(),
                event_id: "deleted-event-2".to_string(),
                target: "https://hooks.acme.com/webhook".to_string(),
                dedupe_key: "webhook-rule:stale-deleted-test".to_string(),
                payload: json!({"schema": "test"}),
                matched_vps: Vec::new(),
                message: "test".to_string(),
                rule_revision_hash: "deleted-test-revision".to_string(),
                signing_secret: None,
                cooldown_until_unix: 0,
                actor_id: Some(operator.operator.id),
            },
        ])
        .await
        .unwrap();
    assert!(stale_dispatch.is_empty());

    assert!(repo.list_webhook_rules(20, None).await.unwrap().is_empty());
    let retained = repo
        .list_webhook_rule_deliveries(20, Some(rule_id), None, None)
        .await
        .unwrap();
    assert_eq!(retained.len(), 1);
    assert_eq!(retained[0].id, created[0].id);
    assert_eq!(retained[0].status, "canceled_disabled");
    assert_eq!(retained[0].error.as_deref(), Some("webhook rule deleted"));
    assert!(repo
        .claim_webhook_rule_deliveries_for_process(&[created[0].id], Uuid::new_v4(), 60)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn webhook_rule_signing_secret_is_redacted_preserved_rotated_and_cleared() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = test_operator();
    let rule_id = Uuid::new_v4();
    let base_request =
        |signing_secret: Option<&str>, clear_signing_secret: bool, target_suffix: &str| {
            crate::model_webhook_rules::CreateWebhookRuleRequest {
                id: Some(rule_id),
                name: "signed-edge-rule".to_string(),
                enabled: true,
                expression: "interval.30sec && tag:edge".to_string(),
                target: format!("https://hooks.acme.com/{target_suffix}"),
                body_template: "{rule.name} {event.kind}".to_string(),
                signing_secret: signing_secret.map(ToOwned::to_owned),
                clear_signing_secret,
                cooldown_secs: Some(60),
                notes: None,
                confirmed: true,
            }
        };

    let created = repo
        .upsert_webhook_rule(
            &base_request(Some("alpha-secret"), false, "create"),
            &operator,
        )
        .await
        .unwrap();
    assert!(created.signing_secret_set);
    assert_eq!(created.signing_secret.as_deref(), Some("alpha-secret"));
    let serialized = serde_json::to_value(&created).unwrap();
    assert_eq!(serialized["signing_secret_set"], true);
    assert!(serialized.get("signing_secret").is_none());

    let preserved = repo
        .upsert_webhook_rule(&base_request(None, false, "preserve"), &operator)
        .await
        .unwrap();
    assert!(preserved.signing_secret_set);
    assert_eq!(preserved.signing_secret.as_deref(), Some("alpha-secret"));
    assert_eq!(preserved.target, "https://hooks.acme.com/preserve");

    let rotated = repo
        .upsert_webhook_rule(
            &base_request(Some("beta-secret"), false, "rotate"),
            &operator,
        )
        .await
        .unwrap();
    assert!(rotated.signing_secret_set);
    assert_eq!(rotated.signing_secret.as_deref(), Some("beta-secret"));

    let cleared = repo
        .upsert_webhook_rule(&base_request(None, true, "clear"), &operator)
        .await
        .unwrap();
    assert!(!cleared.signing_secret_set);
    assert_eq!(cleared.signing_secret, None);
}

#[tokio::test]
async fn webhook_rule_dispatch_can_be_scoped_to_one_rule() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = test_operator();
    if let Repository::Memory(memory) = &repo {
        memory.agents.write().await.push(AgentView {
            id: "edge-a".to_string(),
            display_name: "Edge A".to_string(),
            status: "online".to_string(),
            tags: vec!["edge".to_string()],
            registration_ip: None,
            last_ip: None,
            last_seen_at: None,
            arch: None,
            internal_build_number: 1,
            process_incarnation_id: None,
            stale_since: None,
            stale_reason: None,
            capabilities: AgentCapabilitySnapshot::default(),
        });
    }
    let first_rule_id = Uuid::new_v4();
    let second_enabled_rule_id = Uuid::new_v4();
    let scoped_rule_id = Uuid::new_v4();
    for (id, name) in [
        (first_rule_id, "alpha-webhook"),
        (second_enabled_rule_id, "middle-webhook"),
        (scoped_rule_id, "zulu-webhook"),
    ] {
        repo.upsert_webhook_rule(
            &crate::model_webhook_rules::CreateWebhookRuleRequest {
                id: Some(id),
                name: name.to_string(),
                enabled: id != scoped_rule_id,
                expression: "interval.30sec && tag:edge".to_string(),
                target: format!("https://hooks.acme.com/{name}"),
                body_template: "{rule.name} {event.kind}".to_string(),
                signing_secret: (id == scoped_rule_id).then(|| "scoped-secret".to_string()),
                clear_signing_secret: false,
                cooldown_secs: Some(60),
                notes: None,
                confirmed: true,
            },
            &operator,
        )
        .await
        .unwrap();
    }
    if let Repository::Memory(memory) = &repo {
        let mut rules = memory.webhook_rules.write().await;
        let scoped_rule = rules
            .iter()
            .find(|rule| rule.id == scoped_rule_id)
            .cloned()
            .unwrap();
        rules.extend((0..1_000).map(|index| {
            let mut rule = scoped_rule.clone();
            rule.id = Uuid::new_v4();
            rule.name = format!("filler-webhook-{index:04}");
            rule
        }));
    }
    assert!(
        repo.list_webhook_rules(1_000, None)
            .await
            .unwrap()
            .iter()
            .all(|rule| rule.id != scoped_rule_id),
        "the scoped rule must be outside the broad list cap for this regression"
    );

    let state = alert_test_state(repo);
    let broad_preview = state
        .dispatch_webhook_rules(
            &crate::model_webhook_rules::WebhookRuleDispatchRequest {
                rule_id: None,
                event_kind: "interval.30sec".to_string(),
                event_id: Some("event-1".to_string()),
                limit: Some(1),
                dry_run: Some(true),
                preview_hash: None,
                confirmed: false,
            },
            &operator,
        )
        .await
        .unwrap();
    assert_eq!(broad_preview.len(), 1);
    assert_eq!(broad_preview[0].rule_id, first_rule_id);

    let broad_dispatch = state
        .dispatch_webhook_rules(
            &crate::model_webhook_rules::WebhookRuleDispatchRequest {
                rule_id: None,
                event_kind: "interval.30sec".to_string(),
                event_id: Some("event-1".to_string()),
                limit: Some(1),
                dry_run: Some(false),
                preview_hash: broad_preview[0].review_preview_hash.clone(),
                confirmed: true,
            },
            &operator,
        )
        .await
        .unwrap();
    assert_eq!(broad_dispatch.len(), 1);
    assert_eq!(broad_dispatch[0].rule_id, first_rule_id);
    assert_eq!(broad_dispatch[0].status, "queued");
    if let Repository::Memory(memory) = &state.repo {
        assert!(
            memory.webhook_events.read().await.is_empty(),
            "manual dispatch must not defer broad rule re-evaluation to the worker"
        );
    }

    let scoped_preview = state
        .dispatch_webhook_rules(
            &crate::model_webhook_rules::WebhookRuleDispatchRequest {
                rule_id: Some(scoped_rule_id),
                event_kind: "interval.30sec".to_string(),
                event_id: Some("event-2".to_string()),
                limit: Some(1),
                dry_run: Some(true),
                preview_hash: None,
                confirmed: false,
            },
            &operator,
        )
        .await
        .unwrap();
    assert_eq!(scoped_preview.len(), 1);
    assert_eq!(scoped_preview[0].rule_id, scoped_rule_id);
    assert_eq!(scoped_preview[0].status, "matched_dry_run");
    assert_eq!(
        scoped_preview[0].signing_secret.as_deref(),
        Some("scoped-secret")
    );

    let scoped_review_hash = scoped_preview[0].review_preview_hash.clone();
    let scoped_dispatch = state
        .dispatch_webhook_rules(
            &crate::model_webhook_rules::WebhookRuleDispatchRequest {
                rule_id: Some(scoped_rule_id),
                event_kind: "interval.30sec".to_string(),
                event_id: Some("event-2".to_string()),
                limit: Some(1),
                dry_run: Some(false),
                preview_hash: scoped_review_hash,
                confirmed: true,
            },
            &operator,
        )
        .await
        .unwrap();
    assert_eq!(scoped_dispatch.len(), 1);
    assert_eq!(scoped_dispatch[0].rule_id, scoped_rule_id);
    assert_eq!(scoped_dispatch[0].status, "queued");
}

#[tokio::test]
async fn webhook_rule_dispatch_generated_event_id_can_be_confirmed_when_reused() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = test_operator();
    if let Repository::Memory(memory) = &repo {
        memory.agents.write().await.push(AgentView {
            id: "edge-a".to_string(),
            display_name: "Edge A".to_string(),
            status: "online".to_string(),
            tags: vec!["edge".to_string()],
            registration_ip: None,
            last_ip: None,
            last_seen_at: None,
            arch: None,
            internal_build_number: 1,
            process_incarnation_id: None,
            stale_since: None,
            stale_reason: None,
            capabilities: AgentCapabilitySnapshot::default(),
        });
    }
    repo.upsert_webhook_rule(
        &crate::model_webhook_rules::CreateWebhookRuleRequest {
            id: None,
            name: "generated-event-webhook".to_string(),
            enabled: true,
            expression: "interval.30sec && tag:edge".to_string(),
            target: "https://hooks.acme.com/generated-event-webhook".to_string(),
            body_template: "{rule.name} {event.id}".to_string(),
            signing_secret: None,
            clear_signing_secret: false,
            cooldown_secs: Some(60),
            notes: None,
            confirmed: true,
        },
        &operator,
    )
    .await
    .unwrap();

    let state = alert_test_state(repo);
    let preview = state
        .dispatch_webhook_rules(
            &crate::model_webhook_rules::WebhookRuleDispatchRequest {
                rule_id: None,
                event_kind: "interval.30sec".to_string(),
                event_id: None,
                limit: Some(50),
                dry_run: Some(true),
                preview_hash: None,
                confirmed: false,
            },
            &operator,
        )
        .await
        .unwrap();
    assert_eq!(preview.len(), 1);
    let reviewed_event_id = preview[0].event_id.clone();
    assert!(!reviewed_event_id.is_empty());
    let reviewed_hash = preview[0].review_preview_hash.clone();

    tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;

    let missing_event_id_error = state
        .dispatch_webhook_rules(
            &crate::model_webhook_rules::WebhookRuleDispatchRequest {
                rule_id: None,
                event_kind: "interval.30sec".to_string(),
                event_id: None,
                limit: Some(50),
                dry_run: Some(false),
                preview_hash: reviewed_hash.clone(),
                confirmed: true,
            },
            &operator,
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(missing_event_id_error.contains("webhook_rule_dispatch_event_id_required"));

    let dispatch = state
        .dispatch_webhook_rules(
            &crate::model_webhook_rules::WebhookRuleDispatchRequest {
                rule_id: None,
                event_kind: "interval.30sec".to_string(),
                event_id: Some(reviewed_event_id.clone()),
                limit: Some(50),
                dry_run: Some(false),
                preview_hash: reviewed_hash,
                confirmed: true,
            },
            &operator,
        )
        .await
        .unwrap();
    assert_eq!(dispatch.len(), 1);
    assert_eq!(dispatch[0].event_id, reviewed_event_id);
    assert_eq!(dispatch[0].status, "queued");
    if let Repository::Memory(memory) = &state.repo {
        assert!(memory.webhook_events.read().await.is_empty());
    }
}

fn alert_test_state(repo: Repository) -> AppState {
    AppState {
        repo,
        events: crate::state::WsEventBus::new(4).0,
        internal_token: None,
        gateway: GatewayDispatchClient::default(),
        backup_object_store: None,
        update_release_policy: Default::default(),
        job_output_artifact_min_bytes: 32768,
        artifact_max_bytes: crate::state::DEFAULT_ARTIFACT_MAX_BYTES,
        require_registered_agent_updates: false,
        suite_config_path: std::path::PathBuf::from("config/vpsman.toml"),
        dispatcher_config: crate::state::DispatcherRuntimeConfig::default(),
    }
}

fn metric_policy_rule_request(id: Option<Uuid>, name: &str, severity: &str) -> PolicyRuleRequest {
    PolicyRuleRequest {
        id,
        name: name.to_string(),
        enabled: true,
        rule_kind: crate::model_alert_policies::AlertPolicyRuleKind::Metric,
        evidence_source: "telemetry.combined".to_string(),
        correlation_mode: crate::model_alert_policies::AlertPolicyCorrelationMode::NaturalKey,
        traffic_selector: None,
        trigger_condition_expression: "cpu.load_1 >= 1".to_string(),
        trigger_meta_condition: None,
        resolve_condition_expression: None,
        resolve_meta_condition: None,
        severity: severity.to_string(),
        category: "resource".to_string(),
        title_template: "Resource threshold reached".to_string(),
        detail_template: "CPU load is above the configured threshold".to_string(),
    }
}

fn test_operator() -> AuthContext {
    AuthContext {
        operator: OperatorView {
            id: Uuid::new_v4(),
            username: "test-admin".to_string(),
            role: "admin".to_string(),
            scopes: vec!["*".to_string()],
            preferences: crate::model::OperatorPreferences::default(),
            totp_enabled: false,
            status: "active".to_string(),
            session_refresh_ttl_secs: crate::DEFAULT_REFRESH_TOKEN_TTL_SECS,
            created_at: crate::unix_now().to_string(),
            disabled_at: None,
            deleted_at: None,
        },
        session_id: Some(Uuid::new_v4()),
    }
}
