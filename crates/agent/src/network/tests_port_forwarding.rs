use super::*;
use vpsman_common::{
    pair_port_expressions, port_forwarding_desired_hash, PortForwardProtocol, PortForwardRule,
};

fn config() -> AgentPortForwardingConfig {
    let rules = vec![PortForwardRule {
        pool: Default::default(),
        id: uuid::Uuid::parse_str("018f89ac-a5ec-7d71-a249-7ccddc0a0001").unwrap(),
        revision: 3,
        name: "web".to_string(),
        protocol: PortForwardProtocol::Both,
        target_ip: Some("192.0.2.8".parse().unwrap()),
        mode: PortForwardMode::Dnat,
        address_family: None,
        adapter: None,
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
        pool: Default::default(),
        id: uuid::Uuid::parse_str("018f89ac-a5ec-7d71-a249-7ccddc0a0002").unwrap(),
        revision: 1,
        name: "identity-range".to_string(),
        protocol: PortForwardProtocol::Tcp,
        target_ip: Some("192.0.2.9".parse().unwrap()),
        mode: PortForwardMode::Dnat,
        address_family: None,
        adapter: None,
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

#[tokio::test]
#[ignore = "requires isolated Docker network namespace, nftables and NET_ADMIN"]
async fn bulk_removal_reports_all_native_and_custom_cleanup() {
    assert_eq!(
        std::env::var("VPSMAN_PORT_FORWARD_NATIVE_TEST").as_deref(),
        Ok("1")
    );
    use vpsman_common::{PortForwardAdapterCommands, PortForwardCleanupRule, RuntimeTunnelCommand};
    let root = crate::state_dir::agent_state_dir().unwrap();
    std::fs::create_dir_all(&root).unwrap();
    let command = |script: &str| RuntimeTunnelCommand {
        argv: vec![
            "/bin/sh".into(),
            "-c".into(),
            script.into(),
            "fixture".into(),
            root.join("listener-{rule_id}")
                .to_string_lossy()
                .into_owned(),
        ],
        max_timeout_secs: 5,
        max_output_bytes: 16384,
    };
    let (_, mut consumer) = PortForwardingConsumer::channel("bulk-removal-fixture".into());
    for (modes, removals) in [
        (
            vec![
                PortForwardMode::Dnat,
                PortForwardMode::Redirect,
                PortForwardMode::Dnat,
            ],
            vec![1, 0],
        ),
        (vec![PortForwardMode::CustomAdapter; 3], vec![1, 0]),
        (
            vec![
                PortForwardMode::Dnat,
                PortForwardMode::Redirect,
                PortForwardMode::CustomAdapter,
            ],
            vec![0],
        ),
        (
            vec![
                PortForwardMode::CustomAdapter,
                PortForwardMode::Dnat,
                PortForwardMode::Redirect,
            ],
            vec![1, 0],
        ),
        (
            vec![
                PortForwardMode::Dnat,
                PortForwardMode::CustomAdapter,
                PortForwardMode::CustomAdapter,
                PortForwardMode::Redirect,
            ],
            vec![1, 0],
        ),
    ] {
        let mut desired = AgentPortForwardingConfig {
            schema_version: 2,
            ..Default::default()
        };
        for (index, mode) in modes.into_iter().enumerate() {
            let mut rule = config().rules.remove(0);
            rule.id = uuid::Uuid::new_v4();
            rule.name = format!("bulk-{index}");
            rule.mode = mode;
            rule.masquerade = mode == PortForwardMode::Dnat;
            rule.target_ip = (mode == PortForwardMode::Dnat).then(|| "192.0.2.80".parse().unwrap());
            rule.address_family =
                (mode == PortForwardMode::Redirect).then_some(PortForwardAddressFamily::Both);
            rule.mappings = pair_port_expressions(&(18080 + index).to_string(), "8080").unwrap();
            if mode == PortForwardMode::CustomAdapter {
                rule.adapter = Some(PortForwardAdapterCommands {
                    contract_version: 1,
                    pool_capabilities: None,
                    definition_id: uuid::Uuid::new_v4(),
                    definition_name: "fixture".into(),
                    definition_hash: "fixture".into(),
                    apply: command("printf '%s' '{\"state\":\"applied\"}' > \"$1\""),
                    remove: command("printf '%s' '{\"state\":\"absent\"}' > \"$1\""),
                    status: command("cat \"$1\""),
                });
            }
            desired.rules.push(rule);
        }
        desired.desired_hash = port_forwarding_desired_hash(&desired.rules);
        let native_present = desired
            .rules
            .iter()
            .any(|r| r.mode != PortForwardMode::CustomAdapter);
        let applied = consumer
            .reconcile(
                &desired,
                native_present,
                false,
                false,
                CommandCancelToken::default(),
            )
            .await
            .unwrap();
        assert_eq!(
            applied.status,
            PortForwardRuntimeStatus::Applied,
            "{applied:?}"
        );
        // Cover a single bulk deletion as well as partial removal with a survivor.
        for retain in removals {
            let retired = desired.rules.drain(retain..).collect::<Vec<_>>();
            desired.native_cleanup_pending |= retired
                .iter()
                .any(|r| r.mode != PortForwardMode::CustomAdapter);
            desired.cleanup_rules.extend(
                retired
                    .iter()
                    .filter(|r| r.mode == PortForwardMode::CustomAdapter)
                    .map(|r| PortForwardCleanupRule {
                        rule_id: r.id,
                        revision: r.revision + 1,
                    }),
            );
            desired.desired_hash = native_hash(&desired.rules);
            let removed = consumer
                .reconcile(
                    &desired,
                    native_present,
                    false,
                    false,
                    CommandCancelToken::default(),
                )
                .await
                .unwrap();
            for snapshot in [
                removed,
                consumer
                    .inspect(&desired, 0, CommandCancelToken::default())
                    .await,
            ] {
                eprintln!(
                    "bulk cleanup: {}",
                    serde_json::to_string(&snapshot).unwrap()
                );
                assert!(
                    matches!(
                        snapshot.status,
                        PortForwardRuntimeStatus::Applied | PortForwardRuntimeStatus::Absent
                    ),
                    "{snapshot:?}"
                );
                for receipt in &desired.cleanup_rules {
                    assert!(
                        snapshot.removed_rules.contains(receipt),
                        "missing {receipt:?}: {snapshot:?}"
                    );
                }
                if native_present {
                    assert_eq!(
                        snapshot.native_desired_hash,
                        Some(native_config(&desired).desired_hash)
                    );
                    assert_eq!(
                        snapshot.owned_table_present,
                        Some(
                            desired
                                .rules
                                .iter()
                                .any(|r| r.mode != PortForwardMode::CustomAdapter)
                        )
                    );
                } else {
                    assert_eq!(
                        snapshot.owned_table_present, None,
                        "custom-only cleanup must not inspect nftables"
                    );
                }
            }
            let (_, mut restarted) = PortForwardingConsumer::channel("bulk-removal-fixture".into());
            restarted.probe().await;
            let inspected = restarted
                .inspect(&desired, 0, CommandCancelToken::default())
                .await;
            if desired.rules.is_empty() && native_present {
                assert_eq!(
                    inspected.native_desired_hash.as_deref(),
                    Some(""),
                    "restart must still observe native cleanup"
                );
                let reconciled = restarted
                    .reconcile(&desired, false, false, false, CommandCancelToken::default())
                    .await
                    .unwrap();
                assert_eq!(
                    reconciled.native_desired_hash.as_deref(),
                    Some(""),
                    "accepted cleanup intent must require native evidence after restart"
                );
            }
        }
    }
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

#[test]
fn custom_changes_do_not_change_native_program_or_native_identity() {
    let native = config();
    let mut mixed = native.clone();
    mixed.schema_version = 2;
    let mut custom = native.rules[0].clone();
    custom.id = uuid::Uuid::new_v4();
    custom.mode = PortForwardMode::CustomAdapter;
    custom.target_ip = None;
    custom.masquerade = false;
    let command = vpsman_common::RuntimeTunnelCommand {
        argv: vec!["/bin/true".to_string()],
        max_timeout_secs: 30,
        max_output_bytes: 16 * 1024,
    };
    custom.adapter = Some(vpsman_common::PortForwardAdapterCommands {
        contract_version: 1,
        pool_capabilities: None,
        definition_id: uuid::Uuid::new_v4(),
        definition_name: "service".to_string(),
        definition_hash: "fixture".to_string(),
        apply: command.clone(),
        remove: command.clone(),
        status: command,
    });
    mixed.rules.push(custom);
    mixed.desired_hash = port_forwarding_desired_hash(&mixed.rules);
    assert_eq!(native_config(&native), native_config(&mixed));
    assert_eq!(
        render_apply_script(&native, true).unwrap(),
        render_apply_script(&mixed, true).unwrap()
    );
}

#[test]
fn redirect_both_dispatches_both_families_without_a_return_path_rule() {
    let mut config = config();
    config.schema_version = 2;
    config.rules[0].mode = PortForwardMode::Redirect;
    config.rules[0].target_ip = None;
    config.rules[0].address_family = Some(PortForwardAddressFamily::Both);
    config.rules[0].masquerade = false;
    config.desired_hash = port_forwarding_desired_hash(&config.rules);
    let script = render_apply_script(&config, false).unwrap();
    assert_eq!(
        populated_dispatches(&config.rules),
        vec![
            ("ipv4", "tcp"),
            ("ipv4", "udp"),
            ("ipv6", "tcp"),
            ("ipv6", "udp")
        ]
    );
    assert!(script.contains("redirect to : tcp dport map @pf_0_tcp_fixed"));
    assert!(!script.contains("masquerade"));
    assert!(!script.contains("dnat"));
}

#[test]
fn empty_desired_snapshot_preserves_legacy_hash_and_native_cleanup_proof() {
    let desired = AgentPortForwardingConfig::default();
    let observed = PortForwardRuntimeSnapshot {
        status: PortForwardRuntimeStatus::Absent,
        owned_table_present: Some(false),
        ..PortForwardRuntimeSnapshot::default()
    };
    let merged = merge_snapshot(&desired, &native_config(&desired), observed, Vec::new());
    assert_eq!(merged.status, PortForwardRuntimeStatus::Absent);
    assert_eq!(merged.desired_hash, None);
    assert_eq!(merged.native_desired_hash.as_deref(), Some(""));
    assert_eq!(merged.owned_table_present, Some(false));
    assert!(merged.rules.is_empty());
}

#[test]
fn successful_custom_rule_does_not_hide_failed_native_cleanup() {
    let mut desired = config();
    desired.rules[0].mode = PortForwardMode::CustomAdapter;
    let custom = adapters::runtime_stat(&desired.rules[0], PortForwardRuntimeStatus::Applied);
    let native = native_config(&desired);
    let failed = failed_snapshot(
        &native,
        &PortForwardCapability::default(),
        "native_reconcile_failed",
        "missing nft",
    );
    let merged = merge_snapshot(&desired, &native, failed, vec![custom]);
    assert_eq!(merged.status, PortForwardRuntimeStatus::Failed);
    assert_eq!(
        merged.rules[0].status,
        Some(PortForwardRuntimeStatus::Applied)
    );
    assert_eq!(merged.native_desired_hash, None);
}

#[cfg(target_os = "linux")]
#[path = "tests_port_forwarding_packets.rs"]
mod packets;
