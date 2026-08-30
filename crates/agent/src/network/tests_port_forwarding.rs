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
    assert!(script.contains("tcp dport vmap @pf_dispatch_ipv4_tcp"));
    assert!(script.contains("udp dport vmap @pf_dispatch_ipv4_udp"));
    assert!(script.contains("tcp dport map @pf_0_tcp_fixed"));
    assert!(script.contains("udp dport map @pf_0_udp_fixed"));
    assert!(script.contains("tcp dport map @pf_0_tcp_shift"));
    assert!(script.contains("udp dport map @pf_0_udp_shift"));
    assert_eq!(script.matches("fib daddr type local").count(), 2);
    assert_eq!(script.matches("vpsman-rule:").count(), 2);
    assert!(!script.contains("flush ruleset"));
    assert!(!script.contains("docker"));
    assert!(!script.contains("iptables"));
    assert!(!script.contains("sysctl"));
    assert!(!script.contains(" iifname "));
    assert!(!script.contains(" oifname "));
}

#[test]
fn unchanged_large_port_range_stays_compact() {
    let rules = vec![PortForwardRule {
        id: uuid::Uuid::parse_str("018f89ac-a5ec-7d71-a249-7ccddc0a0002").unwrap(),
        revision: 1,
        name: "identity-range".to_string(),
        protocol: PortForwardProtocol::Tcp,
        target_ip: "192.0.2.9".parse().unwrap(),
        mappings: pair_port_expressions("10000-30000", "10000-30000").unwrap(),
        masquerade: true,
    }];
    let config = AgentPortForwardingConfig {
        desired_hash: port_forwarding_desired_hash(&rules),
        rules,
        ..AgentPortForwardingConfig::default()
    };

    let script = render_apply_script(&config, false).unwrap();
    assert!(script.contains("tcp dport { 10000-30000 } dnat ip to 192.0.2.9"));
    assert!(!script.contains("_shift"));
    assert!(!script.contains("10001 : 10001"));
    assert!(script.len() < 4 * 1024);
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
    assert_eq!(
        normalized_table_structure_hash(&empty),
        normalized_table_structure_hash(&changed_map)
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
    assert!(table_has_ownership_declaration(&owned));
    assert!(!table_is_owned(&comment_only));
    assert!(!table_has_ownership_declaration(&comment_only));
    assert!(!table_is_owned(&foreign));
    assert!(!table_is_owned(&unmarked));
}

#[test]
fn terse_ownership_declaration_does_not_require_hidden_elements() {
    let terse = serde_json::json!({"nftables":[
        {"table":{
            "family":"inet",
            "name":"vpsman_port_forward"
        }},
        {"set":{
            "family":"inet",
            "table":"vpsman_port_forward",
            "name":"vpsman_ownership_v1",
            "type":"mark"
        }}
    ]});
    assert!(!table_is_owned(&terse));
    assert!(table_has_ownership_declaration(&terse));
}

#[test]
fn nft_monitor_ignores_only_live_owned_flow_updates() {
    assert!(nft_event_invalidates_owned_table(
        "add element inet vpsman_port_forward ports { 80 : 8080 }"
    ));
    assert!(nft_event_invalidates_owned_table(
        "delete table inet vpsman_port_forward"
    ));
    assert!(!nft_event_invalidates_owned_table(
        "add element inet vpsman_port_forward owned_flows { 7 timeout 2m }"
    ));
    assert!(!nft_event_invalidates_owned_table(
        "add table inet unrelated"
    ));
}

#[test]
fn nft_monitor_start_owner_is_released_when_spawn_fails() {
    assert!(
        start_nft_monitor(Path::new("/definitely-missing-vpsman-nft-monitor-binary")).is_none()
    );
}
