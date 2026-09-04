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
