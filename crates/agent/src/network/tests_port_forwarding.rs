use super::*;
use vpsman_common::{
    pair_port_expressions, port_forwarding_desired_hash, PortForwardProtocol, PortForwardRule,
};

fn config() -> AgentPortForwardingConfig {
    let rules = vec![PortForwardRule {
        id: uuid::Uuid::parse_str("018f89ac-a5ec-7d71-a249-7ccddc0a0001").unwrap(),
        revision: 3,
        name: "web".to_string(),
        protocol: PortForwardProtocol::Both,
        target_ip: "192.0.2.8".parse().unwrap(),
        mappings: pair_port_expressions("80,1000-1002", "8080,2000-2002").unwrap(),
        masquerade: true,
    }];
    AgentPortForwardingConfig {
        desired_hash: port_forwarding_desired_hash(&rules),
        rules,
        ..AgentPortForwardingConfig::default()
    }
}

#[test]
fn renders_only_the_owned_table_and_local_destination_rules() {
    let script = render_apply_script(&config(), true).unwrap();
    assert!(script.starts_with("delete table inet vpsman_port_forward\n"));
    assert!(script.contains("table inet vpsman_port_forward"));
    assert!(script.contains("fib daddr type local"));
    assert!(script.contains("hook prerouting priority -110"));
    assert!(script.contains("hook output priority -110"));
    assert!(script.contains("hook postrouting priority 90"));
    assert!(script.contains("ct id @owned_flows"));
    assert!(script.contains("tcp dport map @pf_0_tcp_1"));
    assert!(script.contains("udp dport map @pf_0_udp_1"));
    assert!(!script.contains("flush ruleset"));
    assert!(!script.contains("docker"));
    assert!(!script.contains("iptables"));
    assert!(!script.contains("sysctl"));
    assert!(!script.contains(" iifname "));
    assert!(!script.contains(" oifname "));
}

#[test]
fn preserve_source_omits_owned_flow_tracking_and_postrouting() {
    let mut config = config();
    config.rules[0].masquerade = false;
    config.desired_hash = port_forwarding_desired_hash(&config.rules);
    let script = render_apply_script(&config, false).unwrap();
    assert!(!script.contains("owned_flows"));
    assert!(!script.contains("hook postrouting"));
}

#[test]
fn empty_desired_state_deletes_only_the_owned_table() {
    let script = render_apply_script(&AgentPortForwardingConfig::default(), true).unwrap();
    assert_eq!(script, "delete table inet vpsman_port_forward\n");
}

#[test]
fn normalization_ignores_handles_and_counter_values_but_not_structure() {
    let left = serde_json::json!({"nftables":[{"rule":{"handle":4,"expr":[{"counter":{"packets":1,"bytes":4}}]}}]});
    let right = serde_json::json!({"nftables":[{"rule":{"handle":9,"expr":[{"counter":{"packets":8,"bytes":99}}]}}]});
    assert_eq!(normalized_table_hash(&left), normalized_table_hash(&right));
    let changed = serde_json::json!({"nftables":[{"rule":{"expr":[]}}]});
    assert_ne!(
        normalized_table_hash(&left),
        normalized_table_hash(&changed)
    );
}

#[test]
fn normalization_ignores_live_owned_flow_entries_only() {
    let empty = serde_json::json!({
        "nftables": [
            {"set": {
                "family": "inet",
                "table": "vpsman_port_forward",
                "name": "owned_flows",
                "type": "ct_id"
            }},
            {"map": {
                "family": "inet",
                "table": "vpsman_port_forward",
                "name": "pf_0_tcp_0",
                "elem": [{"elem": {"val": 80}}, {"elem": {"val": 8080}}]
            }}
        ]
    });
    let populated = serde_json::json!({
        "nftables": [
            {"set": {
                "family": "inet",
                "table": "vpsman_port_forward",
                "name": "owned_flows",
                "type": "ct_id",
                "elem": [{"elem": {"val": 123, "expires": 119}}]
            }},
            {"element": {
                "family": "inet",
                "table": "vpsman_port_forward",
                "name": "owned_flows",
                "elem": [{"elem": {"val": 456, "expires": 118}}]
            }},
            {"map": {
                "family": "inet",
                "table": "vpsman_port_forward",
                "name": "pf_0_tcp_0",
                "elem": [{"elem": {"val": 80}}, {"elem": {"val": 8080}}]
            }}
        ]
    });
    assert_eq!(
        normalized_table_hash(&empty),
        normalized_table_hash(&populated)
    );

    let changed_map = serde_json::json!({
        "nftables": [
            {"set": {
                "family": "inet",
                "table": "vpsman_port_forward",
                "name": "owned_flows",
                "type": "ct_id"
            }},
            {"map": {
                "family": "inet",
                "table": "vpsman_port_forward",
                "name": "pf_0_tcp_0",
                "elem": [{"elem": {"val": 80}}, {"elem": {"val": 9090}}]
            }}
        ]
    });
    assert_ne!(
        normalized_table_hash(&empty),
        normalized_table_hash(&changed_map)
    );
}

#[test]
fn ownership_requires_the_exact_table_marker() {
    let owned = serde_json::json!({"nftables":[
        {"table":{
            "family":"inet",
            "name":"vpsman_port_forward"
        }},
        {"set":{
            "family":"inet",
            "table":"vpsman_port_forward",
            "name":"vpsman_ownership_v1",
            "type":"mark",
            "elem":[1448104781]
        }}
    ]});
    let comment_only = serde_json::json!({"nftables":[{"table":{
        "family":"inet",
        "name":"vpsman_port_forward",
        "comment":"vpsman-owned desired=abcd"
    }}]});
    let foreign = serde_json::json!({"nftables":[{"table":{
        "family":"inet",
        "name":"vpsman_port_forward",
        "comment":"operator-owned"
    }}]});
    let unmarked = serde_json::json!({"nftables":[{"table":{
        "family":"inet",
        "name":"vpsman_port_forward"
    }}]});

    assert!(table_is_owned(&owned));
    assert!(!table_is_owned(&comment_only));
    assert!(!table_is_owned(&foreign));
    assert!(!table_is_owned(&unmarked));
}
