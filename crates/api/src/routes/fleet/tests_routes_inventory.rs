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
use vpsman_common::GATEWAY_CONTROL_BATCH_MAX_ITEMS;
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

    let bounded = BulkResolveManyRequest {
        items: (0..BULK_RESOLVE_MANY_ITEM_LIMIT)
            .map(|index| BulkResolveManyItem {
                selector_expression: format!("id:v-{index}"),
            })
            .collect(),
    };
    validate_bulk_resolve_many_request(&bounded, &["fleet:read".to_string()])
        .expect("one fleet-sized selector batch must be accepted");

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

    let bounded = BulkDeleteAgentsRequest {
        items: (0..GATEWAY_CONTROL_BATCH_MAX_ITEMS)
            .map(|index| BulkDeleteAgentItem {
                client_id: format!("vps-{index}"),
                privilege_assertion: None,
            })
            .collect(),
        confirmed: true,
        reason: None,
    };
    validate_bulk_delete_agents_request(&bounded).expect("one fleet-sized delete is valid");

    let oversized = BulkDeleteAgentsRequest {
        items: (0..=GATEWAY_CONTROL_BATCH_MAX_ITEMS)
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

    let bounded = BulkAgentSuspensionRequest {
        action: AgentSuspensionAction::Unsuspend,
        client_ids: (0..GATEWAY_CONTROL_BATCH_MAX_ITEMS)
            .map(|index| format!("vps-{index}"))
            .collect(),
        confirmed: true,
        reason: None,
    };
    validate_bulk_agent_suspension_request(&bounded)
        .expect("one fleet-sized suspension update is valid");

    let too_many = BulkAgentSuspensionRequest {
        action: AgentSuspensionAction::Unsuspend,
        client_ids: (0..=GATEWAY_CONTROL_BATCH_MAX_ITEMS)
            .map(|index| format!("vps-{index}"))
            .collect(),
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
