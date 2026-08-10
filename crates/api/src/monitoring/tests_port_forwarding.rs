use axum::{
    extract::{Path, State},
    Json,
};
use tokio::sync::broadcast;
use vpsman_common::{
    pair_port_expressions, PortForwardProtocol, PortForwardRuleRuntimeStat,
    PortForwardRuntimeSnapshot, PortForwardRuntimeStatus,
};

use crate::{
    model::{AgentView, AuthContext, OperatorPreferences, OperatorView},
    model_port_forwarding::{
        CreatePortForwardRuleRequest, PortForwardBulkAction, PortForwardBulkItem,
        PortForwardRuleListItem, UpdatePortForwardRuleRequest, UpdateTargetHostname,
    },
    repository::{MemoryState, Repository},
    state::AppState,
};

#[tokio::test]
async fn desired_rules_reject_overlap_and_stale_mutations() {
    let repo = port_forward_repo().await;
    let operator = port_forward_operator();
    let created = repo
        .create_port_forward_rule(&create_request("web", "80", "8080", true), &operator)
        .await
        .unwrap();

    let overlap = repo
        .create_port_forward_rule(
            &CreatePortForwardRuleRequest {
                name: "conflict".to_string(),
                protocol: PortForwardProtocol::Both,
                ..create_request("ignored", "80", "9090", true)
            },
            &operator,
        )
        .await
        .unwrap_err();
    assert!(overlap.to_string().contains("overlap"));

    let stale = repo
        .set_port_forward_rule_enabled(created.id, created.revision + 1, false, &operator)
        .await
        .unwrap_err();
    assert!(stale.to_string().contains("snapshot_stale"));
    assert_eq!(
        repo.list_port_forward_rules().await.unwrap()[0].runtime_status,
        "pending"
    );
}

#[tokio::test]
async fn memory_management_list_preserves_typed_rule_identity() {
    let repo = port_forward_repo().await;
    let created = repo
        .create_port_forward_rule(
            &create_request("memory-visible", "18080", "8080", false),
            &port_forward_operator(),
        )
        .await
        .unwrap();
    let items = repo.list_port_forward_rule_items().await.unwrap();
    assert!(matches!(
        items.as_slice(),
        [crate::model_port_forwarding::PortForwardRuleListItem::Rule(rule)]
            if rule.id == created.id
    ));
}

#[tokio::test]
async fn hostname_context_round_trips_and_stays_out_of_agent_desired_state() {
    let repo = port_forward_repo().await;
    let operator = port_forward_operator();
    let mut request = create_request("resolved-web", "443", "8443", true);
    request.target_hostname = Some(" App.Internal. ".to_string());

    let created = repo
        .create_port_forward_rule(&request, &operator)
        .await
        .unwrap();
    assert_eq!(created.target_hostname.as_deref(), Some("app.internal"));
    let config = repo
        .port_forwarding_config_for_client("edge-a")
        .await
        .unwrap();
    assert_eq!(config.rules[0].target_ip, created.target_ip);
    assert!(!serde_json::to_string(&config)
        .unwrap()
        .contains("app.internal"));

    let disabled = repo
        .set_port_forward_rule_enabled(created.id, created.revision, false, &operator)
        .await
        .unwrap();
    assert_eq!(disabled.target_hostname.as_deref(), Some("app.internal"));

    let mut invalid = create_request("invalid-hostname", "444", "8444", false);
    invalid.target_hostname = Some("192.0.2.8".to_string());
    assert!(repo
        .create_port_forward_rule(&invalid, &operator)
        .await
        .unwrap_err()
        .to_string()
        .contains("target_hostname_invalid"));
}

#[test]
fn update_hostname_json_distinguishes_omitted_null_and_string_values() {
    let mut request = serde_json::json!({
        "expected_revision": 1,
        "name": "resolved-web",
        "protocol": "tcp",
        "target_ip": "192.0.2.8",
        "mappings": pair_port_expressions("443", "8443").unwrap(),
        "masquerade": true,
        "enabled": false,
        "confirmed": false,
    });
    let omitted: UpdatePortForwardRuleRequest = serde_json::from_value(request.clone()).unwrap();
    assert_eq!(omitted.target_hostname, UpdateTargetHostname::Preserve);

    request["target_hostname"] = serde_json::Value::Null;
    let cleared: UpdatePortForwardRuleRequest = serde_json::from_value(request.clone()).unwrap();
    assert_eq!(cleared.target_hostname, UpdateTargetHostname::Clear);

    request["target_hostname"] = serde_json::json!(" New.App.Internal. ");
    let replaced: UpdatePortForwardRuleRequest = serde_json::from_value(request).unwrap();
    assert_eq!(
        replaced.target_hostname,
        UpdateTargetHostname::Replace(" New.App.Internal. ".to_string())
    );
}

#[tokio::test]
async fn mutation_routes_return_normalized_hostname_context_for_editing() {
    let state = port_forward_test_state(port_forward_repo().await);
    let headers = crate::test_auth_headers(&state).await;
    let mut create = create_request("route-resolved-web", "1443", "8443", false);
    create.target_hostname = Some(" App.Internal. ".to_string());

    let (_, Json(created)) = crate::routes_port_forwarding::create_port_forward_rule(
        State(state.clone()),
        headers.clone(),
        Json(create),
    )
    .await
    .unwrap();
    let PortForwardRuleListItem::Rule(created) = created.rule else {
        panic!("created port-forward rule must be typed");
    };
    assert_eq!(created.target_hostname.as_deref(), Some("app.internal"));

    let Json(preserved) = crate::routes_port_forwarding::update_port_forward_rule(
        State(state.clone()),
        headers.clone(),
        Path(created.id),
        Json(UpdatePortForwardRuleRequest {
            expected_revision: created.revision,
            name: created.name.clone(),
            protocol: created.protocol,
            target_ip: "192.0.2.9".parse().unwrap(),
            target_hostname: UpdateTargetHostname::Preserve,
            mappings: created.mappings.clone(),
            masquerade: created.masquerade,
            enabled: false,
            confirmed: false,
        }),
    )
    .await
    .unwrap();
    let PortForwardRuleListItem::Rule(preserved) = preserved.rule else {
        panic!("updated port-forward rule must be typed");
    };
    assert_eq!(preserved.target_hostname.as_deref(), Some("app.internal"));
    assert_eq!(
        preserved.target_ip,
        "192.0.2.9".parse::<std::net::IpAddr>().unwrap()
    );

    let Json(updated) = crate::routes_port_forwarding::update_port_forward_rule(
        State(state.clone()),
        headers.clone(),
        Path(preserved.id),
        Json(UpdatePortForwardRuleRequest {
            expected_revision: preserved.revision,
            name: preserved.name.clone(),
            protocol: preserved.protocol,
            target_ip: preserved.target_ip,
            target_hostname: UpdateTargetHostname::Replace(" New.App.Internal. ".to_string()),
            mappings: preserved.mappings.clone(),
            masquerade: preserved.masquerade,
            enabled: false,
            confirmed: false,
        }),
    )
    .await
    .unwrap();
    let PortForwardRuleListItem::Rule(updated) = updated.rule else {
        panic!("updated port-forward rule must be typed");
    };
    assert_eq!(updated.target_hostname.as_deref(), Some("new.app.internal"));
    assert_eq!(
        updated.target_ip,
        "192.0.2.9".parse::<std::net::IpAddr>().unwrap()
    );

    let Json(cleared) = crate::routes_port_forwarding::update_port_forward_rule(
        State(state.clone()),
        headers.clone(),
        Path(updated.id),
        Json(UpdatePortForwardRuleRequest {
            expected_revision: updated.revision,
            name: updated.name.clone(),
            protocol: updated.protocol,
            target_ip: updated.target_ip,
            target_hostname: UpdateTargetHostname::Clear,
            mappings: updated.mappings.clone(),
            masquerade: updated.masquerade,
            enabled: false,
            confirmed: false,
        }),
    )
    .await
    .unwrap();
    let PortForwardRuleListItem::Rule(cleared) = cleared.rule else {
        panic!("updated port-forward rule must be typed");
    };
    assert_eq!(cleared.target_hostname, None);

    let invalid = crate::routes_port_forwarding::update_port_forward_rule(
        State(state),
        headers,
        Path(cleared.id),
        Json(UpdatePortForwardRuleRequest {
            expected_revision: cleared.revision,
            name: cleared.name.clone(),
            protocol: cleared.protocol,
            target_ip: cleared.target_ip,
            target_hostname: UpdateTargetHostname::Replace("192.0.2.9".to_string()),
            mappings: cleared.mappings.clone(),
            masquerade: cleared.masquerade,
            enabled: false,
            confirmed: false,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(invalid.status, axum::http::StatusCode::BAD_REQUEST);
    assert_eq!(invalid.code, "hostname_invalid");
}

#[tokio::test]
async fn runtime_state_is_hash_bound_and_cleanup_is_evidence_bound() {
    let repo = port_forward_repo().await;
    let operator = port_forward_operator();
    let created = repo
        .create_port_forward_rule(&create_request("web", "80", "8080", true), &operator)
        .await
        .unwrap();
    let initial = repo
        .port_forwarding_config_for_client("edge-a")
        .await
        .unwrap();
    repo.record_port_forward_runtime_snapshot(
        "edge-a",
        &PortForwardRuntimeSnapshot {
            status: PortForwardRuntimeStatus::Applied,
            desired_hash: Some(initial.desired_hash.clone()),
            observed_hash: Some("table-hash-1".to_string()),
            nft_version: Some("nftables test".to_string()),
            ipv4_forwarding_enabled: Some(false),
            rules: vec![PortForwardRuleRuntimeStat {
                rule_id: created.id,
                revision: created.revision,
                nat_matches: 42,
            }],
            observed_unix: 100,
            ..PortForwardRuntimeSnapshot::default()
        },
    )
    .await
    .unwrap();
    let observed = repo.list_port_forward_rules().await.unwrap().remove(0);
    assert_eq!(observed.runtime_status, "applied_warning");
    assert_eq!(observed.nat_matches, 42);
    assert_eq!(observed.forwarding_enabled, Some(false));

    let updated = repo
        .update_port_forward_rule(
            created.id,
            &UpdatePortForwardRuleRequest {
                expected_revision: created.revision,
                name: created.name.clone(),
                protocol: created.protocol,
                target_ip: "192.0.2.9".parse().unwrap(),
                target_hostname: UpdateTargetHostname::Preserve,
                mappings: created.mappings.clone(),
                masquerade: created.masquerade,
                enabled: true,
                confirmed: true,
            },
            &operator,
        )
        .await
        .unwrap();
    assert_eq!(
        repo.list_port_forward_rules().await.unwrap()[0].runtime_status,
        "pending"
    );

    let current = repo
        .port_forwarding_config_for_client("edge-a")
        .await
        .unwrap();
    repo.record_port_forward_runtime_snapshot(
        "edge-a",
        &PortForwardRuntimeSnapshot {
            status: PortForwardRuntimeStatus::Applied,
            desired_hash: Some(current.desired_hash.clone()),
            observed_hash: Some("table-hash-2".to_string()),
            ipv4_forwarding_enabled: Some(true),
            observed_unix: 101,
            ..PortForwardRuntimeSnapshot::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(
        repo.list_port_forward_rules().await.unwrap()[0].runtime_status,
        "applied"
    );

    let deleted = repo
        .delete_port_forward_rule(created.id, updated.revision, None, &operator)
        .await
        .unwrap();
    assert_eq!(deleted.desired_status, "removal_pending");
    repo.record_port_forward_runtime_snapshot(
        "edge-a",
        &PortForwardRuntimeSnapshot {
            status: PortForwardRuntimeStatus::Applied,
            desired_hash: Some(current.desired_hash),
            observed_hash: Some("stale-table".to_string()),
            observed_unix: 102,
            ..PortForwardRuntimeSnapshot::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(repo.list_port_forward_rules().await.unwrap().len(), 1);

    repo.record_port_forward_runtime_snapshot(
        "edge-a",
        &PortForwardRuntimeSnapshot {
            status: PortForwardRuntimeStatus::Absent,
            observed_unix: 103,
            ..PortForwardRuntimeSnapshot::default()
        },
    )
    .await
    .unwrap();
    assert!(repo.list_port_forward_rules().await.unwrap().is_empty());
    assert!(!repo
        .port_forwarding_blocks_agent_delete("edge-a")
        .await
        .unwrap());
}

#[tokio::test]
async fn disable_waits_for_current_cleanup_and_tombstones_cannot_reapply() {
    let repo = port_forward_repo().await;
    let operator = port_forward_operator();
    let created = repo
        .create_port_forward_rule(&create_request("ssh", "2222", "22", true), &operator)
        .await
        .unwrap();
    let disabled = repo
        .set_port_forward_rule_enabled(created.id, created.revision, false, &operator)
        .await
        .unwrap();
    assert_eq!(
        repo.list_port_forward_rules().await.unwrap()[0].runtime_status,
        "pending"
    );
    repo.record_port_forward_runtime_snapshot(
        "edge-a",
        &PortForwardRuntimeSnapshot {
            status: PortForwardRuntimeStatus::Absent,
            observed_unix: 200,
            ..PortForwardRuntimeSnapshot::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(
        repo.list_port_forward_rules().await.unwrap()[0].runtime_status,
        "disabled"
    );

    let deleted = repo
        .delete_port_forward_rule(created.id, disabled.revision, None, &operator)
        .await
        .unwrap();
    let error = repo
        .bulk_mutate_port_forward_rules(
            PortForwardBulkAction::Reapply,
            &[PortForwardBulkItem {
                id: deleted.id,
                expected_revision: deleted.revision,
            }],
            None,
            &operator,
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("not_active"));
}

#[tokio::test]
async fn deleting_never_enabled_draft_requires_no_agent_cleanup_evidence() {
    let repo = port_forward_repo().await;
    let operator = port_forward_operator();
    let draft = repo
        .create_port_forward_rule(
            &create_request("unsupported-draft", "8443", "443", false),
            &operator,
        )
        .await
        .unwrap();

    let deleted = repo
        .delete_port_forward_rule(draft.id, draft.revision, None, &operator)
        .await
        .unwrap();

    assert!(deleted.removal_confirmed_at.is_some());
    assert!(repo.list_port_forward_rules().await.unwrap().is_empty());
    assert!(!repo
        .port_forwarding_blocks_agent_delete("edge-a")
        .await
        .unwrap());
}

#[tokio::test]
async fn rejected_memory_bulk_mutation_rolls_back_every_rule() {
    let repo = port_forward_repo().await;
    let operator = port_forward_operator();
    let first = repo
        .create_port_forward_rule(&create_request("draft-a", "443", "8443", false), &operator)
        .await
        .unwrap();
    let second = repo
        .create_port_forward_rule(&create_request("draft-b", "443", "9443", false), &operator)
        .await
        .unwrap();

    let error = repo
        .bulk_mutate_port_forward_rules(
            PortForwardBulkAction::Enable,
            &[
                PortForwardBulkItem {
                    id: first.id,
                    expected_revision: first.revision,
                },
                PortForwardBulkItem {
                    id: second.id,
                    expected_revision: second.revision,
                },
            ],
            None,
            &operator,
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("overlap"));

    let rules = repo.list_port_forward_rules().await.unwrap();
    assert_eq!(rules.len(), 2);
    assert!(rules.iter().all(|rule| !rule.enabled && rule.revision == 1));
}

#[tokio::test]
async fn transient_inspection_failure_keeps_last_known_table_deletion_guard() {
    let repo = port_forward_repo().await;
    let operator = port_forward_operator();
    let created = repo
        .create_port_forward_rule(&create_request("web", "443", "8443", true), &operator)
        .await
        .unwrap();
    let desired = repo
        .port_forwarding_config_for_client("edge-a")
        .await
        .unwrap();
    repo.record_port_forward_runtime_snapshot(
        "edge-a",
        &PortForwardRuntimeSnapshot {
            status: PortForwardRuntimeStatus::Applied,
            owned_table_present: Some(true),
            desired_hash: Some(desired.desired_hash),
            observed_hash: Some("known-table".to_string()),
            observed_unix: 300,
            ..PortForwardRuntimeSnapshot::default()
        },
    )
    .await
    .unwrap();
    repo.set_port_forward_rule_enabled(created.id, created.revision, false, &operator)
        .await
        .unwrap();

    repo.record_port_forward_runtime_snapshot(
        "edge-a",
        &PortForwardRuntimeSnapshot {
            status: PortForwardRuntimeStatus::Failed,
            error_code: Some("inspection_failed".to_string()),
            observed_unix: 301,
            ..PortForwardRuntimeSnapshot::default()
        },
    )
    .await
    .unwrap();
    assert!(repo
        .port_forwarding_blocks_agent_delete("edge-a")
        .await
        .unwrap());

    repo.record_port_forward_runtime_snapshot(
        "edge-a",
        &PortForwardRuntimeSnapshot {
            status: PortForwardRuntimeStatus::Absent,
            owned_table_present: Some(false),
            observed_unix: 302,
            ..PortForwardRuntimeSnapshot::default()
        },
    )
    .await
    .unwrap();
    assert!(!repo
        .port_forwarding_blocks_agent_delete("edge-a")
        .await
        .unwrap());
}

#[tokio::test]
async fn agent_delete_archives_clean_disabled_drafts() {
    let repo = port_forward_repo().await;
    let operator = port_forward_operator();
    repo.create_port_forward_rule(&create_request("draft", "8443", "443", false), &operator)
        .await
        .unwrap();

    repo.delete_agent("edge-a", Some("retired"), &operator)
        .await
        .unwrap();

    assert!(repo.list_port_forward_rules().await.unwrap().is_empty());
}

#[tokio::test]
async fn agent_delete_rejects_enabled_port_forwarding_state() {
    let repo = port_forward_repo().await;
    let operator = port_forward_operator();
    repo.create_port_forward_rule(&create_request("web", "443", "8443", true), &operator)
        .await
        .unwrap();

    let error = repo
        .delete_agent("edge-a", Some("retired"), &operator)
        .await
        .unwrap_err();

    assert!(error
        .to_string()
        .contains("agent_port_forwarding_cleanup_required"));
    assert_eq!(repo.list_port_forward_rules().await.unwrap().len(), 1);
}

fn create_request(
    name: &str,
    incoming: &str,
    target: &str,
    enabled: bool,
) -> CreatePortForwardRuleRequest {
    CreatePortForwardRuleRequest {
        client_id: "edge-a".to_string(),
        name: name.to_string(),
        protocol: PortForwardProtocol::Tcp,
        target_ip: "192.0.2.8".parse().unwrap(),
        target_hostname: None,
        mappings: pair_port_expressions(incoming, target).unwrap(),
        masquerade: true,
        enabled,
        confirmed: enabled,
    }
}

fn port_forward_operator() -> AuthContext {
    AuthContext {
        operator: OperatorView {
            id: uuid::Uuid::new_v4(),
            username: "port-forward-test".to_string(),
            role: "admin".to_string(),
            scopes: vec!["*".to_string()],
            preferences: OperatorPreferences::default(),
            totp_enabled: false,
            status: "active".to_string(),
            session_refresh_ttl_secs: crate::DEFAULT_REFRESH_TOKEN_TTL_SECS,
            created_at: crate::unix_now().to_string(),
            disabled_at: None,
            deleted_at: None,
        },
        session_id: Some(uuid::Uuid::new_v4()),
    }
}

async fn port_forward_repo() -> Repository {
    let memory = MemoryState::default();
    memory.agents.write().await.push(AgentView {
        id: "edge-a".to_string(),
        display_name: "edge-a".to_string(),
        status: "offline".to_string(),
        tags: Vec::new(),
        registration_ip: None,
        last_ip: None,
        last_seen_at: None,
        arch: Some("x86_64".to_string()),
        internal_build_number: 1,
        process_incarnation_id: None,
        stale_since: None,
        stale_reason: None,
        capabilities: vpsman_common::AgentCapabilitySnapshot::default(),
    });
    Repository::Memory(memory)
}

fn port_forward_test_state(repo: Repository) -> AppState {
    let (events, _) = broadcast::channel(16);
    AppState {
        repo,
        events,
        internal_token: None,
        gateway: crate::gateway_client::GatewayDispatchClient::test_privilege_auto_approve(),
        backup_object_store: None,
        update_release_policy: Default::default(),
        fleet_alert_policy: Default::default(),
        job_output_artifact_min_bytes: crate::state::DEFAULT_ARTIFACT_MAX_BYTES,
        artifact_max_bytes: crate::state::DEFAULT_ARTIFACT_MAX_BYTES,
        require_registered_agent_updates: false,
        suite_config_path: std::path::PathBuf::from("config/vpsman-test-missing.toml"),
        dispatcher_config: Default::default(),
    }
}
