use super::{
    peer_client_ids_for_deleted_agent, runtime_config_patch_validation_error,
    telemetry_network_rate_limit_or_default, validate_legacy_tag_name_for_cleanup,
    validate_persisted_tag_name, validate_telemetry_network_rate_query,
    validate_telemetry_rollup_query,
};
use crate::model::{TelemetryNetworkRateQuery, TelemetryRollupQuery};
use axum::http::StatusCode;

#[test]
fn runtime_config_patch_reports_server_managed_port_forwarding() {
    let error = runtime_config_patch_validation_error(anyhow::anyhow!(
        "runtime_config_patch_managed_port_forwarding_forbidden"
    ));
    assert_eq!(error.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        error.code,
        "runtime_config_patch_managed_port_forwarding_forbidden"
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
    validate_legacy_tag_name_for_cleanup("provider:").unwrap();
    validate_legacy_tag_name_for_cleanup(":alpha").unwrap();
    validate_legacy_tag_name_for_cleanup("role::edge").unwrap();
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
