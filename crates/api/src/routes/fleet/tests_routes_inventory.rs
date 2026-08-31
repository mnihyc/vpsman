use super::{
    agent_delete_invalidation_event, agent_suspension_invalidation_event,
    agent_suspension_rejection_error, agent_suspension_rejection_message,
    canonical_lifecycle_reason, delete_agent_rejection_error, delete_agent_rejection_message,
    peer_client_ids_for_deleted_agent, telemetry_network_rate_limit_or_default,
    validate_bulk_agent_suspension_request, validate_bulk_delete_agents_request,
    validate_bulk_resolve_many_request, validate_persisted_tag_name, validate_suspend_agent_status,
    validate_telemetry_network_rate_query, validate_telemetry_rollup_query,
    BULK_RESOLVE_MANY_ITEM_LIMIT,
};
use crate::model::{
    AgentSuspensionAction, BulkAgentSuspensionRequest, BulkDeleteAgentItem,
    BulkDeleteAgentsRequest, BulkResolveManyItem, BulkResolveManyRequest,
    TelemetryNetworkRateQuery, TelemetryRollupQuery,
};
use axum::http::StatusCode;
#[test]
fn selector_batch_is_bounded_normalized_unique_and_ordered() {
    let request = BulkResolveManyRequest {
        items: vec![
            BulkResolveManyItem {
                selector_expression: " status:online ".to_string(),
            },
            BulkResolveManyItem {
                selector_expression: "tag:edge".to_string(),
            },
        ],
    };
    let validated = validate_bulk_resolve_many_request(&request, &["fleet:read".to_string()])
        .expect("all expressions validate before the resolution read");
    assert_eq!(validated[0].0, "status:online");
    assert_eq!(validated[1].0, "tag:edge");

    let duplicate = BulkResolveManyRequest {
        items: vec![
            BulkResolveManyItem {
                selector_expression: "tag:edge".to_string(),
            },
            BulkResolveManyItem {
                selector_expression: " tag:edge ".to_string(),
            },
        ],
    };
    assert_eq!(
        validate_bulk_resolve_many_request(&duplicate, &["fleet:read".to_string()])
            .unwrap_err()
            .code,
        "selector_batch_duplicate_item"
    );

    let oversized = BulkResolveManyRequest {
        items: (0..=BULK_RESOLVE_MANY_ITEM_LIMIT)
            .map(|index| BulkResolveManyItem {
                selector_expression: format!("id:v-{index}"),
            })
            .collect(),
    };
    assert_eq!(
        validate_bulk_resolve_many_request(&oversized, &["fleet:read".to_string()])
            .unwrap_err()
            .code,
        "selector_batch_items_invalid"
    );
}

#[test]
fn persisted_tags_reject_inner_selector_prefixes() {
    validate_persisted_tag_name("provider:alpha").unwrap();
    validate_persisted_tag_name("country:US").unwrap();
    validate_persisted_tag_name("region:legacy-name").unwrap();

    assert!(validate_persisted_tag_name("id:edge-a").is_err());
    assert!(validate_persisted_tag_name("name:edge-a").is_err());
    assert!(validate_persisted_tag_name("provider:").is_err());
    assert!(validate_persisted_tag_name(":alpha").is_err());
    assert!(validate_persisted_tag_name("role::edge").is_err());
}

#[test]
fn telemetry_network_rates_allow_fleet_scale_limits() {
    assert_eq!(telemetry_network_rate_limit_or_default(None), 100);
    assert_eq!(telemetry_network_rate_limit_or_default(Some(5_000)), 5_000);
    assert_eq!(telemetry_network_rate_limit_or_default(Some(50_000)), 5_000);
}

#[test]
fn telemetry_queries_accept_adaptive_minute_aligned_spans() {
    for bucket_secs in [60, 120, 300, 86_400] {
        validate_telemetry_rollup_query(&TelemetryRollupQuery {
            limit: None,
            client_id: None,
            bucket_secs: Some(bucket_secs),
            latest: false,
        })
        .unwrap();
        validate_telemetry_network_rate_query(&TelemetryNetworkRateQuery {
            limit: None,
            client_id: None,
            interface: None,
            bucket_secs: Some(bucket_secs),
            latest: false,
        })
        .unwrap();
    }

    for bucket_secs in [-60, 0, 59, 61] {
        assert!(validate_telemetry_rollup_query(&TelemetryRollupQuery {
            limit: None,
            client_id: None,
            bucket_secs: Some(bucket_secs),
            latest: false,
        })
        .is_err());
        assert!(
            validate_telemetry_network_rate_query(&TelemetryNetworkRateQuery {
                limit: None,
                client_id: None,
                interface: None,
                bucket_secs: Some(bucket_secs),
                latest: false,
            })
            .is_err()
        );
    }
}

#[test]
fn deleting_agent_collects_each_declared_tunnel_peer_once() {
    let peers = peer_client_ids_for_deleted_agent(
        "edge-a",
        [
            ("edge-a".to_string(), "edge-b".to_string()),
            ("edge-c".to_string(), "edge-a".to_string()),
            ("edge-a".to_string(), "edge-b".to_string()),
            ("edge-c".to_string(), "edge-d".to_string()),
        ],
    );
    assert_eq!(peers.into_iter().collect::<Vec<_>>(), ["edge-b", "edge-c"]);
}

#[test]
fn bulk_delete_validation_is_bounded_unique_and_preserves_request_order() {
    let valid = BulkDeleteAgentsRequest {
        items: vec![
            BulkDeleteAgentItem {
                client_id: "vps-b".to_string(),
                privilege_assertion: None,
            },
            BulkDeleteAgentItem {
                client_id: "vps-a".to_string(),
                privilege_assertion: None,
            },
        ],
        confirmed: true,
        reason: Some("retired".to_string()),
    };
    validate_bulk_delete_agents_request(&valid).unwrap();
    assert_eq!(valid.items[0].client_id, "vps-b");
    assert_eq!(valid.items[1].client_id, "vps-a");

    let duplicate = BulkDeleteAgentsRequest {
        items: vec![
            BulkDeleteAgentItem {
                client_id: "vps-a".to_string(),
                privilege_assertion: None,
            },
            BulkDeleteAgentItem {
                client_id: "vps-a".to_string(),
                privilege_assertion: None,
            },
        ],
        confirmed: true,
        reason: None,
    };
    let error = validate_bulk_delete_agents_request(&duplicate).unwrap_err();
    assert_eq!(error.code, "agent_delete_targets_duplicate");

    let oversized = BulkDeleteAgentsRequest {
        items: (0..501)
            .map(|index| BulkDeleteAgentItem {
                client_id: format!("vps-{index}"),
                privilege_assertion: None,
            })
            .collect(),
        confirmed: true,
        reason: None,
    };
    let error = validate_bulk_delete_agents_request(&oversized).unwrap_err();
    assert_eq!(error.code, "agent_delete_targets_invalid");
}

#[test]
fn deletion_invalidation_is_absent_for_zero_success_and_exact_for_partial_success() {
    assert!(agent_delete_invalidation_event(&[]).is_none());

    let affected = vec!["vps-b".to_string(), "vps-a".to_string()];
    assert!(matches!(
        agent_delete_invalidation_event(&affected),
        Some(crate::model::WsEvent::FleetStateInvalidated)
    ));
}

#[test]
fn exact_client_transaction_failures_are_actionable_and_singletons_remain_server_errors() {
    let suspension_message = agent_suspension_rejection_message("agent_suspension_target_failed");
    assert!(suspension_message.contains("retry this VPS"));
    assert!(suspension_message.contains("processed independently"));
    let suspension_error = agent_suspension_rejection_error("agent_suspension_target_failed");
    assert_eq!(suspension_error.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(suspension_error.code, "agent_suspension_target_failed");

    let deletion_message = delete_agent_rejection_message("agent_delete_target_failed");
    assert!(deletion_message.contains("retry this VPS"));
    assert!(deletion_message.contains("processed independently"));
    let deletion_error = delete_agent_rejection_error("agent_delete_target_failed");
    assert_eq!(deletion_error.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(deletion_error.code, "agent_delete_target_failed");
}

#[test]
fn exact_client_batches_are_not_canceled_after_partial_commits() {
    let source = include_str!("routes_inventory.rs");

    assert!(!source.contains("AGENT_SUSPENSION_DB_BUDGET_SECS"));
    assert!(!source.contains("AGENT_DELETE_DB_BUDGET_SECS"));
    assert!(!source.contains("\"agent_suspension_timeout\""));
    assert!(!source.contains("\"agent_delete_timeout\""));
    assert!(source.contains("CLIENT_LIFECYCLE_FENCE_CONTROL_ATTEMPT_SECS"));
    assert!(source.contains("CLIENT_LIFECYCLE_FENCE_LEASE_SECS"));
    assert!(source.contains(
        "CLIENT_LIFECYCLE_FENCE_RENEWAL_SECS: u64 = CLIENT_LIFECYCLE_FENCE_LEASE_SECS / 3"
    ));
}

#[test]
fn lifecycle_finalizers_are_bounded_and_database_transactions_are_never_canceled() {
    let source = include_str!("routes_inventory.rs");
    let (_, promote) = source
        .split_once("async fn promote_committed(&mut self) -> bool")
        .expect("committed promotion owner");
    let (promote, _) = promote
        .split_once("async fn compensate(&self, reason: &str)")
        .expect("committed promotion boundary");
    let stop = promote
        .find("self.stop_renewal().await")
        .expect("renewal stop");
    let attempt = promote
        .find("self.promote_once().await")
        .expect("one promotion");
    assert!(stop < attempt);
    assert!(!promote.contains("loop {"));

    for owner_name in [
        "async fn mutate_delete_agent_target_owned(",
        "async fn mutate_agent_suspension_target_owned(",
    ] {
        let (_, owner) = source.split_once(owner_name).expect("exact target owner");
        let (owner, _) = owner.split_once("async fn mutate_").unwrap_or((owner, ""));
        assert!(!owner.contains("tokio::select!"));
    }
}

#[test]
fn singleton_delete_delegates_and_exact_targets_finalize_before_the_next_target() {
    let source = include_str!("routes_inventory.rs");
    let (_, singleton) = source
        .split_once("pub(crate) async fn delete_agent(")
        .expect("delete route");
    let (singleton, _) = singleton
        .split_once("pub(crate) async fn bulk_delete_agents(")
        .expect("bulk delete route boundary");
    assert!(singleton.contains("mutate_delete_agents("));
    assert!(!singleton.contains("agent_by_id("));
    assert!(!singleton.contains("AgentUpdated"));

    let (_, wrapper) = source
        .split_once("async fn mutate_delete_agents(")
        .expect("delete batch owner");
    let (owner, _) = wrapper
        .split_once("fn agent_delete_invalidation_event(")
        .expect("bulk delete owner boundary");
    assert!(owner.contains("tokio::spawn(mutate_delete_agent_target_owned("));
    assert_eq!(owner.matches(".verify_privileges(").count(), 1);
    assert!(owner.contains("for client_id in approved_client_ids"));
    assert!(!owner.contains(".disconnect_sessions("));
    assert_eq!(
        owner
            .matches("dispatch_runtime_config_for_clients(")
            .count(),
        1
    );

    let (_, exact_owner) = source
        .split_once("async fn mutate_delete_agent_target_owned(")
        .expect("service-owned exact delete task");
    let (exact_owner, _) = exact_owner
        .split_once("async fn mutate_delete_agents(")
        .expect("exact delete owner boundary");
    assert!(exact_owner.contains("GatewayClientDispatchFencePurpose::Deletion"));
    assert!(exact_owner.contains(".delete_agent_target("));
    assert!(exact_owner.contains("move || async move { commit_proof.verify().await }"));
    assert!(exact_owner.contains("fence.promote_committed().await"));
    assert!(exact_owner.contains(".disconnect_session_if_fence_owned("));
    assert!(exact_owner.contains("CLIENT_LIFECYCLE_FENCE_CONTROL_ATTEMPT_SECS"));
    assert_eq!(
        owner
            .matches("invalidate_fleet_telemetry_read_cache()")
            .count(),
        1
    );
}

#[test]
fn suspension_status_eligibility_is_exact() {
    for status in ["never", "disconnected", "offline", "stale"] {
        validate_suspend_agent_status(status).unwrap();
    }
    for status in ["online", "suspended", "revoked", "deleted"] {
        assert!(validate_suspend_agent_status(status).is_err(), "{status}");
    }
}

#[test]
fn bulk_suspension_validation_is_bounded_unique_and_order_preserving() {
    let valid = BulkAgentSuspensionRequest {
        action: AgentSuspensionAction::Suspend,
        client_ids: vec!["vps-b".to_string(), "vps-a".to_string()],
        confirmed: true,
        reason: Some("  maintenance  ".to_string()),
    };
    validate_bulk_agent_suspension_request(&valid).unwrap();
    assert_eq!(valid.client_ids, ["vps-b", "vps-a"]);

    let duplicate = BulkAgentSuspensionRequest {
        client_ids: vec!["vps-a".to_string(), "vps-a".to_string()],
        ..valid
    };
    let error = validate_bulk_agent_suspension_request(&duplicate).unwrap_err();
    assert_eq!(error.code, "agent_suspension_targets_duplicate");

    let too_many = BulkAgentSuspensionRequest {
        action: AgentSuspensionAction::Unsuspend,
        client_ids: (0..501).map(|index| format!("vps-{index}")).collect(),
        confirmed: true,
        reason: None,
    };
    let error = validate_bulk_agent_suspension_request(&too_many).unwrap_err();
    assert_eq!(error.code, "agent_suspension_targets_invalid");

    let unsuspend_reason = BulkAgentSuspensionRequest {
        action: AgentSuspensionAction::Unsuspend,
        client_ids: vec!["vps-a".to_string()],
        confirmed: true,
        reason: Some("not part of the unsuspend contract".to_string()),
    };
    let error = validate_bulk_agent_suspension_request(&unsuspend_reason).unwrap_err();
    assert_eq!(error.code, "agent_unsuspend_reason_invalid");
}

#[test]
fn lifecycle_reasons_retain_the_existing_trim_and_empty_canonicalization() {
    assert_eq!(
        canonical_lifecycle_reason(Some("  planned maintenance  ")),
        Some("planned maintenance".to_string())
    );
    assert_eq!(canonical_lifecycle_reason(Some("      ")), None);
    assert_eq!(canonical_lifecycle_reason(None), None);
}

#[test]
fn suspension_invalidation_is_absent_for_zero_success_and_exact_for_partial_success() {
    assert!(agent_suspension_invalidation_event(&[]).is_none());

    let affected = vec!["vps-b".to_string(), "vps-a".to_string()];
    assert!(matches!(
        agent_suspension_invalidation_event(&affected),
        Some(crate::model::WsEvent::FleetStateInvalidated)
    ));
}

#[test]
fn singleton_suspension_routes_delegate_without_inventory_preflight_reads() {
    let source = include_str!("routes_inventory.rs");
    let (_, suspend) = source
        .split_once("pub(crate) async fn suspend_agent(")
        .expect("suspend route");
    let (suspend, remaining) = suspend
        .split_once("pub(crate) async fn unsuspend_agent(")
        .expect("unsuspend route");
    let (unsuspend, _) = remaining
        .split_once("pub(crate) async fn bulk_agent_suspensions(")
        .expect("bulk route boundary");
    for route in [suspend, unsuspend] {
        assert!(route.contains("mutate_agent_suspensions("));
        assert!(!route.contains("agent_by_id("));
        assert!(!route.contains("AgentUpdated"));
    }

    let (_, bulk_owner) = source
        .split_once("async fn mutate_agent_suspensions(")
        .expect("bulk suspension owner");
    let (bulk_owner, _) = bulk_owner
        .split_once("fn agent_suspension_invalidation_event(")
        .expect("bulk suspension owner boundary");
    assert!(bulk_owner.contains("tokio::spawn(mutate_agent_suspension_target_owned("));
    assert_eq!(
        bulk_owner
            .matches("invalidate_fleet_telemetry_read_cache()")
            .count(),
        1
    );
}

#[test]
fn exact_suspension_owner_preserves_suspend_and_unsuspend_business_ownership() {
    let source = include_str!("routes_inventory.rs");
    let (_, owner) = source
        .split_once("async fn mutate_agent_suspension_target_owned(")
        .expect("exact suspension owner");
    let (owner, _) = owner
        .split_once("async fn mutate_agent_suspensions(")
        .expect("exact suspension owner boundary");
    assert!(owner.contains("GatewayClientDispatchFencePurpose::Suspension"));
    assert!(owner.contains("action == AgentSuspensionAction::Suspend"));
    assert!(owner.contains("action == AgentSuspensionAction::Unsuspend"));
    assert!(owner.contains(".mutate_agent_suspension_target("));
    assert!(owner.contains("commit_proof.verify().await?"));
    assert!(owner.contains("fence.promote_committed().await"));
    assert!(owner.contains("fence.clear_committed(\"db_authoritative_unsuspend\")"));
    assert!(owner.contains("fence.stop_renewal().await"));
    assert!(owner.contains(".compensate("));
    assert!(owner.contains("\"suspension_transition_not_committed\""));
    let prepare = owner
        .find("ClientDispatchFenceLease::prepare(")
        .expect("pretransaction exact owner");
    let mutation = owner
        .find(".mutate_agent_suspension_target(")
        .expect("database lifecycle mutation");
    assert!(prepare < mutation);
    let (_, repository_result) = owner
        .split_once("let outcome = match repository_outcome {")
        .expect("repository result owner");
    let (_, repository_error) = repository_result
        .split_once("Err(error) => {")
        .expect("repository error owner");
    let (repository_error, _) = repository_error
        .split_once("match &outcome {")
        .expect("repository error owner boundary");
    assert!(repository_error.contains("fence.stop_renewal().await"));
    assert!(!repository_error.contains("fence.compensate("));
    assert!(!owner.contains(".disconnect_session("));
    assert!(!owner.contains("tokio::select!"));
    assert!(!source.contains("reconcile_unsuspended_gateway_route"));
}
