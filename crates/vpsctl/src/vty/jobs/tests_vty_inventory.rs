use super::{
    fleet_alerts_path, gateway_sessions_path, is_vty_gateway_sessions_command,
    is_vty_inventory_command, parse_vty_inventory_command, telemetry_network_rates_path,
    telemetry_rollups_path, telemetry_tunnels_path, VtyInventoryCommand,
};

const TYPED_ALERT_RULE_JSON: &str = r#"{"name":"offline","enabled":true,"rule_kind":"state","evidence_source":"agent.status","correlation_mode":"natural_key","trigger_condition_expression":"evidence.status = offline","resolve_condition_expression":"evidence.status = online","resolve_meta_condition":{"kind":"sustained","seconds":60},"severity":"critical","category":"agent_status","title_template":"Agent offline","detail_template":"{subject.display_name} is offline"}"#;

#[test]
fn parses_typed_alert_policy_json_with_vty_quoting() {
    assert_eq!(
        parse_vty_inventory_command(&format!(
            "alert-policy-preview --name offline-agents --selector tag:edge --rule-json='{TYPED_ALERT_RULE_JSON}' --notes 'reviewed by ops'"
        ))
        .unwrap(),
        VtyInventoryCommand::AlertPolicyPreview {
            name: "offline-agents".to_string(),
            selector: "tag:edge".to_string(),
            rule_json: vec![TYPED_ALERT_RULE_JSON.to_string()],
            enabled: true,
            notes: Some("reviewed by ops".to_string()),
        }
    );
    assert_eq!(
        parse_vty_inventory_command(&format!(
            "alert-policy-upsert --name offline-agents --selector tag:edge --rule-json '{TYPED_ALERT_RULE_JSON}' --confirmed"
        ))
        .unwrap(),
        VtyInventoryCommand::AlertPolicyUpsert {
            name: "offline-agents".to_string(),
            selector: "tag:edge".to_string(),
            rule_json: vec![TYPED_ALERT_RULE_JSON.to_string()],
            enabled: true,
            notes: None,
            confirmed: true,
        }
    );
}

#[test]
fn builds_typed_alert_policy_request() {
    let request = crate::commands_inventory::alert_policy_request(
        crate::commands_inventory::AlertPolicyWriteOptions {
            name: "offline-agents".to_string(),
            selector: Some("tag:edge".to_string()),
            rule_json: vec![TYPED_ALERT_RULE_JSON.to_string()],
            enabled: true,
            notes: None,
            file: None,
            confirmed: false,
        },
        None,
    )
    .unwrap();
    let rule = &request["rules"][0];
    assert_eq!(
        rule,
        &serde_json::from_str::<serde_json::Value>(TYPED_ALERT_RULE_JSON).unwrap()
    );
    assert!(rule.get("trigger_meta_condition").is_none());
}

#[test]
fn recognizes_inventory_commands() {
    assert!(is_vty_inventory_command(
        "config-presets --behavior host_metrics"
    ));
    assert!(is_vty_inventory_command(
        "config-source-set --behavior host_metrics"
    ));
    assert!(is_vty_inventory_command(
        "config-preset-update --preset-id 11111111-1111-4111-8111-111111111111"
    ));
    assert!(is_vty_inventory_command(
        "config-sources --client-id edge-a"
    ));
    assert!(is_vty_inventory_command("fleet-alerts --severity warning"));
    assert!(is_vty_inventory_command(
        "fleet-alert-export --include-muted"
    ));
    assert!(is_vty_inventory_command(
        "fleet-alert-states --state acknowledged"
    ));
    assert!(is_vty_inventory_command(
        "fleet-alert-state-update --alert-id agent_status:agent:abc --action acknowledge --confirmed"
    ));
    assert!(is_vty_inventory_command(
        "fleet-alert-notification-channels --delivery-kind webhook"
    ));
    assert!(is_vty_inventory_command(
        "fleet-alert-notification-channel-upsert --name edge-audit"
    ));
    assert!(is_vty_inventory_command(
        "fleet-alert-notifications --status queued"
    ));
    assert!(is_vty_inventory_command(
        "fleet-alert-notification-dispatch --dry-run"
    ));
    assert!(is_vty_inventory_command(
        "fleet-alert-notification-process --dry-run"
    ));
    assert!(is_vty_inventory_command("config-render --client-id edge-a"));
    assert!(is_vty_inventory_command("bulk-resolve edge bgp"));
    assert!(is_vty_inventory_command(
        "telemetry-rollups --client-id edge-a"
    ));
    assert!(is_vty_inventory_command(
        "telemetry-network-rates --interface eth0"
    ));
    assert!(!is_vty_inventory_command("job-create /bin/true tag:edge"));
    assert!(is_vty_gateway_sessions_command(
        "gateway-sessions --limit 20"
    ));
}

#[test]
fn parses_inventory_commands() {
    assert_eq!(
        parse_vty_inventory_command("bulk-resolve edge bgp").unwrap(),
        VtyInventoryCommand::BulkResolve {
            tags: vec!["edge".to_string(), "bgp".to_string()],
        }
    );
    assert_eq!(
        parse_vty_inventory_command("config-presets --behavior=host_metrics").unwrap(),
        VtyInventoryCommand::ConfigPresets {
            behavior: Some("host_metrics".to_string()),
        }
    );
    assert_eq!(
        parse_vty_inventory_command(
            "config-preset-create --behavior=process_inventory --name=host-proc --definition-json='{\"source\":\"linux_procfs\",\"proc_root\":\"/host/proc\"}'",
        )
        .unwrap(),
        VtyInventoryCommand::ConfigPresetCreate {
            behavior: "process_inventory".to_string(),
            name: "host-proc".to_string(),
            description: None,
            definition: serde_json::json!({
                "source": "linux_procfs",
                "proc_root": "/host/proc"
            }),
        }
    );
    assert_eq!(
        parse_vty_inventory_command(
            "config-preset-clone --preset-id=11111111-1111-4111-8111-111111111111 --name=copy --description copied",
        )
        .unwrap(),
        VtyInventoryCommand::ConfigPresetClone {
            preset_id: "11111111-1111-4111-8111-111111111111".to_string(),
            name: "copy".to_string(),
            description: Some("copied".to_string()),
        }
    );
    assert_eq!(
        parse_vty_inventory_command(
            "config-preset-preview --preset-id=11111111-1111-4111-8111-111111111111 --definition-json='{\"source\":\"linux_procfs\",\"proc_root\":\"/host/proc\"}'",
        )
        .unwrap(),
        VtyInventoryCommand::ConfigPresetPreview {
            preset_id: "11111111-1111-4111-8111-111111111111".to_string(),
            description: None,
            clear_description: false,
            definition: serde_json::json!({"source": "linux_procfs", "proc_root": "/host/proc"}),
        }
    );
    assert_eq!(
        parse_vty_inventory_command(
            "config-preset-update --preset-id 11111111-1111-4111-8111-111111111111 --clear-description --definition-json='{\"source\":\"linux_procfs\",\"proc_root\":\"/host/proc\"}' --preview-hash aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa --confirmed",
        )
        .unwrap(),
        VtyInventoryCommand::ConfigPresetUpdate {
            preset_id: "11111111-1111-4111-8111-111111111111".to_string(),
            description: None,
            clear_description: true,
            definition: serde_json::json!({"source": "linux_procfs", "proc_root": "/host/proc"}),
            preview_hash: Some(
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string()
            ),
            confirmed: true,
        }
    );
    assert_eq!(
        parse_vty_inventory_command(
            "config-source-set --behavior process_inventory --preset-id 11111111-1111-4111-8111-111111111111 --selector provider:alpha&&country:US --client edge-a --tag bgp --preview-hash bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb --confirmed",
        )
        .unwrap(),
        VtyInventoryCommand::ConfigSourceChange {
            action: "set".to_string(),
            behavior: "process_inventory".to_string(),
            preset_id: Some("11111111-1111-4111-8111-111111111111".to_string()),
            selector: Some("provider:alpha&&country:US".to_string()),
            clients: vec!["edge-a".to_string()],
            tags: vec!["bgp".to_string()],
            preview_hash: Some(
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string()
            ),
            confirmed: true,
        }
    );
    assert_eq!(
        parse_vty_inventory_command(
            "config-source-reset --behavior=process_inventory --client=edge/a"
        )
        .unwrap(),
        VtyInventoryCommand::ConfigSourceChange {
            action: "reset".to_string(),
            behavior: "process_inventory".to_string(),
            preset_id: None,
            selector: None,
            clients: vec!["edge/a".to_string()],
            tags: Vec::new(),
            preview_hash: None,
            confirmed: false,
        }
    );
    assert_eq!(
        parse_vty_inventory_command("config-render --client-id=edge/a --format=json").unwrap(),
        VtyInventoryCommand::ConfigRender {
            client_id: "edge/a".to_string(),
            format: "json".to_string(),
        }
    );
    assert_eq!(
        parse_vty_inventory_command(
            "config-sources --client-id=edge/a --behavior=process_inventory"
        )
        .unwrap(),
        VtyInventoryCommand::ConfigSources {
            client_id: Some("edge/a".to_string()),
            behavior: Some("process_inventory".to_string()),
        }
    );
    assert_eq!(
        parse_vty_inventory_command(
            "fleet-alerts --limit 25 --client-id edge/a --severity warning"
        )
        .unwrap(),
        VtyInventoryCommand::FleetAlerts {
            limit: 25,
            client_id: Some("edge/a".to_string()),
            severity: Some("warning".to_string()),
            category: None,
            operator_state: None,
            include_muted: false,
        }
    );
    assert_eq!(
        parse_vty_inventory_command(
            "fleet-alert-export --limit 25 --category agent_status --operator-state muted --include-muted"
        )
        .unwrap(),
        VtyInventoryCommand::FleetAlertExport {
            limit: 25,
            client_id: None,
            severity: None,
            category: Some("agent_status".to_string()),
            operator_state: Some("muted".to_string()),
            include_muted: true,
        }
    );
    assert_eq!(
        parse_vty_inventory_command("fleet-alert-states --limit 25 --state acknowledged").unwrap(),
        VtyInventoryCommand::FleetAlertStates {
            limit: 25,
            state: Some("acknowledged".to_string()),
        }
    );
    assert_eq!(
        parse_vty_inventory_command(
            "fleet-alert-state-update --alert-id agent_status:agent:abc --action mute --muted-for-secs 600 --reason maintenance --confirmed"
        )
        .unwrap(),
        VtyInventoryCommand::FleetAlertStateUpdate {
            alert_id: "agent_status:agent:abc".to_string(),
            action: "mute".to_string(),
            muted_for_secs: Some(600),
            reason: Some("maintenance".to_string()),
            confirmed: true,
        }
    );
    assert_eq!(
        parse_vty_inventory_command(
            "fleet-alert-notification-channels --limit=20 --enabled=true --scope-kind=tag --scope-value=edge --delivery-kind=webhook"
        )
        .unwrap(),
        VtyInventoryCommand::FleetAlertNotificationChannels {
            limit: 20,
            enabled: Some(true),
            scope_kind: Some("tag".to_string()),
            scope_value: Some("edge".to_string()),
            delivery_kind: Some("webhook".to_string()),
        }
    );
    assert_eq!(
        parse_vty_inventory_command(
            "fleet-alert-notification-channel-upsert --name edge-webhook --scope-kind tag --scope-value edge --min-severity warning --categories agent_status,network --operator-states open,escalated --delivery-kind webhook --target https://hooks.example/vpsman --cooldown-secs 600 --notes page-edge --confirmed"
        )
        .unwrap(),
        VtyInventoryCommand::FleetAlertNotificationChannelUpsert {
            name: "edge-webhook".to_string(),
            scope_kind: "tag".to_string(),
            scope_value: Some("edge".to_string()),
            min_severity: Some("warning".to_string()),
            categories: vec!["agent_status".to_string(), "network".to_string()],
            operator_states: vec!["open".to_string(), "escalated".to_string()],
            delivery_kind: "webhook".to_string(),
            target: "https://hooks.example/vpsman".to_string(),
            cooldown_secs: Some(600),
            enabled: true,
            notes: Some("page-edge".to_string()),
            confirmed: true,
        }
    );
    assert_eq!(
        parse_vty_inventory_command(
            "fleet-alert-notifications --limit 20 --alert-id agent_status:agent:abc --status queued"
        )
        .unwrap(),
        VtyInventoryCommand::FleetAlertNotifications {
            limit: 20,
            channel_id: None,
            alert_id: Some("agent_status:agent:abc".to_string()),
            status: Some("queued".to_string()),
        }
    );
    assert_eq!(
        parse_vty_inventory_command(
            "fleet-alert-notification-dispatch --limit 25 --category agent_status --include-muted --dry-run"
        )
        .unwrap(),
        VtyInventoryCommand::FleetAlertNotificationDispatch {
            limit: 25,
            client_id: None,
            severity: None,
            category: Some("agent_status".to_string()),
            operator_state: None,
            include_muted: true,
            dry_run: true,
            preview_hash: None,
            confirmed: false,
        }
    );
    assert_eq!(
        parse_vty_inventory_command(
            "fleet-alert-notification-process --limit 25 --status failed --delivery-kind webhook --preview-hash 1111111111111111111111111111111111111111111111111111111111111111 --confirmed"
        )
        .unwrap(),
        VtyInventoryCommand::FleetAlertNotificationProcess {
            limit: 25,
            status: Some("failed".to_string()),
            delivery_kind: Some("webhook".to_string()),
            dry_run: false,
            preview_hash: Some("1111111111111111111111111111111111111111111111111111111111111111".to_string()),
            confirmed: true,
        }
    );
    assert_eq!(
        parse_vty_inventory_command(
            "telemetry-rollups --limit 20 --client-id edge/a --bucket-secs 300"
        )
        .unwrap(),
        VtyInventoryCommand::TelemetryRollups {
            limit: 20,
            client_id: Some("edge/a".to_string()),
            bucket_secs: Some(300),
            latest: false,
        }
    );
    assert_eq!(
        parse_vty_inventory_command(
            "telemetry-network-rates --limit 20 --client-id edge/a --interface eth0 --bucket-secs 300",
        )
        .unwrap(),
        VtyInventoryCommand::TelemetryNetworkRates {
            limit: 20,
            client_id: Some("edge/a".to_string()),
            interface: Some("eth0".to_string()),
            bucket_secs: Some(300),
            latest: false,
        }
    );
    assert_eq!(
        parse_vty_inventory_command(
            "telemetry-network-rates --limit 5000 --client-id edge/a --interface eth0 --bucket-secs 60 --latest",
        )
        .unwrap(),
        VtyInventoryCommand::TelemetryNetworkRates {
            limit: 5000,
            client_id: Some("edge/a".to_string()),
            interface: Some("eth0".to_string()),
            bucket_secs: Some(60),
            latest: true,
        }
    );
    assert_eq!(
        parse_vty_inventory_command(
            "telemetry-tunnels --limit 20 --client-id edge/a --interface tun0"
        )
        .unwrap(),
        VtyInventoryCommand::TelemetryTunnels {
            limit: 20,
            client_id: Some("edge/a".to_string()),
            interface: Some("tun0".to_string()),
        }
    );
}

#[test]
fn rejects_invalid_inventory_commands() {
    assert!(parse_vty_inventory_command("agent-tag edge-a").is_err());
    assert!(parse_vty_inventory_command("config-preset-create --name x").is_err());
    assert!(parse_vty_inventory_command("config-preset-clone --name x").is_err());
    assert!(parse_vty_inventory_command("config-preset-preview --confirmed").is_err());
    assert!(parse_vty_inventory_command(
        "config-preset-update --description x --clear-description"
    )
    .is_err());
    assert!(parse_vty_inventory_command("config-source-set --behavior x").is_err());
    assert!(parse_vty_inventory_command(
        "config-source-set --behavior process_inventory --preset-id 11111111-1111-4111-8111-111111111111 --client edge-a --confirmed"
    )
    .is_err());
    assert!(parse_vty_inventory_command(
        "config-source-reset --behavior process_inventory --client edge-a --preview-hash aaaa"
    )
    .is_err());
    assert!(parse_vty_inventory_command(
        "config-preset-update --preset-id 11111111-1111-4111-8111-111111111111 --definition-json='{\"source\":\"interface_counters\"}' --confirmed"
    )
    .is_err());
    assert!(parse_vty_inventory_command(
        "config-source-reset --behavior x --preset-id 11111111-1111-4111-8111-111111111111"
    )
    .is_err());
    assert!(parse_vty_inventory_command("unknown").is_err());
    assert!(gateway_sessions_path("gateway-sessions --limit=0").is_err());
    assert!(gateway_sessions_path("gateway-sessions extra").is_err());
    assert!(
        parse_vty_inventory_command("telemetry-network-rates --limit 5001 --bucket-secs 60")
            .is_err()
    );
    assert_eq!(
        telemetry_rollups_path(10, Some("edge/a"), Some(60), false),
        "/api/v1/telemetry/rollups?limit=10&client_id=edge%2Fa&bucket_secs=60"
    );
    assert_eq!(
        telemetry_network_rates_path(10, Some("edge/a"), Some("eth/0"), Some(60), false),
        "/api/v1/telemetry/network-rates?limit=10&client_id=edge%2Fa&interface=eth%2F0&bucket_secs=60"
    );
    assert_eq!(
        telemetry_network_rates_path(5000, Some("edge/a"), Some("eth/0"), Some(60), true),
        "/api/v1/telemetry/network-rates?limit=5000&client_id=edge%2Fa&interface=eth%2F0&bucket_secs=60&latest=true"
    );
    assert_eq!(
        telemetry_tunnels_path(10, Some("edge/a"), Some("tun/0")),
        "/api/v1/telemetry/tunnels?limit=10&client_id=edge%2Fa&interface=tun%2F0"
    );
    assert_eq!(
        super::config_presets_path(Some("host/metrics")),
        "/api/v1/configuration-presets?behavior=host%2Fmetrics"
    );
    assert_eq!(
        super::config_sources_path(Some("edge/a"), Some("host/metrics")),
        "/api/v1/configuration-sources?client_id=edge%2Fa&behavior=host%2Fmetrics"
    );
    assert_eq!(
        super::config_render_path("edge/a"),
        "/api/v1/effective-agent-config?client_id=edge%2Fa"
    );
    assert!(parse_vty_inventory_command("config-render --format xml").is_err());
    assert_eq!(
        crate::commands_inventory::configuration_source_selector(
            Some("provider:alpha && country:US"),
            &["edge".to_string()]
        )
        .unwrap(),
        "(provider:alpha && country:US) || (tag:edge)"
    );
    assert_eq!(
        crate::commands_inventory::configuration_source_preview_target_ids(&serde_json::json!({
            "targets": [
                {"client_id": "edge-b"},
                {"client_id": "edge-a"},
                {"client_id": "edge-b"}
            ]
        }))
        .unwrap(),
        vec!["edge-a".to_string(), "edge-b".to_string()]
    );
    assert!(
        crate::commands_inventory::require_matching_reviewed_preview_hash(
            Some("reviewed"),
            "changed",
            "config-source-set"
        )
        .unwrap_err()
        .to_string()
        .contains("rerun without --confirmed")
    );
    assert!(parse_vty_inventory_command("fleet-alerts --severity noisy").is_err());
    assert!(parse_vty_inventory_command("fleet-alerts --limit=0").is_err());
    assert!(parse_vty_inventory_command("fleet-alerts --operator-state noisy").is_err());
    assert!(parse_vty_inventory_command(
        "fleet-alert-state-update --alert-id agent_status:agent:abc --action mute"
    )
    .is_err());
    assert!(parse_vty_inventory_command("fleet-alert-notification-dispatch").is_err());
    assert!(parse_vty_inventory_command("fleet-alert-notification-process").is_err());
    assert!(parse_vty_inventory_command(
        "fleet-alert-notification-process --status delivered --dry-run"
    )
    .is_err());
    assert!(parse_vty_inventory_command(
        "fleet-alert-notification-channel-upsert --scope-kind tag --scope-value edge --delivery-kind webhook --target x --confirmed"
    )
    .is_err());
    assert_eq!(
        fleet_alerts_path(
            10,
            Some("edge/a"),
            Some("critical"),
            Some("agent_status"),
            Some("muted"),
            true
        ),
        "/api/v1/fleet-alerts?limit=10&client_id=edge%2Fa&severity=critical&category=agent_status&operator_state=muted&include_muted=true"
    );
    assert_eq!(
        super::fleet_alert_export_path(10, None, None, Some("agent_status"), None, true),
        "/api/v1/fleet-alerts/export?limit=10&category=agent_status&include_muted=true"
    );
    assert_eq!(
        super::fleet_alert_states_path(10, Some("muted")),
        "/api/v1/fleet-alert-states?limit=10&state=muted"
    );
    assert_eq!(
        super::fleet_alert_notification_channels_path(
            10,
            Some(true),
            Some("tag"),
            Some("edge/a"),
            Some("webhook")
        ),
        "/api/v1/fleet-alert-notification-channels?limit=10&enabled=true&scope_kind=tag&scope_value=edge%2Fa&delivery_kind=webhook"
    );
    assert_eq!(
        super::fleet_alert_notifications_path(
            10,
            Some("11111111-1111-4111-8111-111111111111"),
            Some("agent_status:agent:abc"),
            Some("queued")
        ),
        "/api/v1/fleet-alert-notifications?limit=10&channel_id=11111111-1111-4111-8111-111111111111&alert_id=agent_status%3Aagent%3Aabc&status=queued"
    );
    assert!(parse_vty_inventory_command("telemetry-rollups --bucket-secs 1").is_err());
    assert!(parse_vty_inventory_command("telemetry-rollups --bucket-secs 61").is_err());
    assert!(
        parse_vty_inventory_command("telemetry-network-rates --interface '' --bucket-secs 1")
            .is_err()
    );
    assert!(parse_vty_inventory_command("telemetry-tunnels --limit=0").is_err());
}

#[test]
fn parses_explicit_tag_confirmations() {
    assert_eq!(
        parse_vty_inventory_command("tag-create prod --confirmed").unwrap(),
        VtyInventoryCommand::TagCreate {
            name: "prod".to_string(),
            confirmed: true,
        }
    );
    assert_eq!(
        parse_vty_inventory_command("agent-tag edge-a prod --confirmed").unwrap(),
        VtyInventoryCommand::AgentTag {
            client_id: "edge-a".to_string(),
            tag: "prod".to_string(),
            confirmed: true,
        }
    );
}
