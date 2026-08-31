use vpsman_common::pair_port_expressions;

use crate::model_port_forwarding::{UpdatePortForwardRuleRequest, UpdateTargetHostname};

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

#[test]
fn bulk_port_forwarding_batches_validation_and_runtime_dispatch() {
    let source = include_str!("../routes/network/routes_port_forwarding.rs");
    let bulk = source
        .split("pub(crate) async fn bulk_mutate_port_forward_rules")
        .nth(1)
        .and_then(|source| {
            source
                .split("pub(crate) async fn resolve_network_hostname")
                .next()
        })
        .expect("bulk port-forwarding route body");
    assert!(bulk.contains("require_agents(&state, &client_ids, true).await?"));
    assert_eq!(
        bulk.matches("dispatch_runtime_config_for_clients(").count(),
        1
    );
    assert!(!bulk.contains("sync_client("));

    let validation = source
        .split("async fn require_agents")
        .nth(1)
        .and_then(|source| source.split("async fn sync_client").next())
        .expect("batched port-forwarding validation helper");
    assert!(validation.contains("list_agents_for_client_ids(&requested)"));
    assert!(!validation.contains(".list_agents()"));
}
