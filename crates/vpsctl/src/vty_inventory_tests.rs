use super::{
    fleet_alerts_path, gateway_sessions_path, is_vty_gateway_sessions_command,
    is_vty_inventory_command, parse_vty_inventory_command, telemetry_network_rates_path,
    telemetry_rollups_path, telemetry_tunnels_path, VtyInventoryCommand,
};

#[test]
fn recognizes_inventory_commands() {
    assert!(is_vty_inventory_command(
        "data-source-presets --domain telemetry_metrics_source"
    ));
    assert!(is_vty_inventory_command(
        "data-source-preset-assign --domain telemetry_metrics_source"
    ));
    assert!(is_vty_inventory_command(
        "data-source-preset-update --preset-id 11111111-1111-4111-8111-111111111111"
    ));
    assert!(is_vty_inventory_command(
        "data-source-status --client-id edge-a"
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
    assert!(is_vty_inventory_command(
        "data-source-hot-config --client-id edge-a"
    ));
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
        parse_vty_inventory_command("data-source-presets --domain=telemetry_metrics_source")
            .unwrap(),
        VtyInventoryCommand::DataSourcePresets {
            domain: Some("telemetry_metrics_source".to_string()),
        }
    );
    assert_eq!(
        parse_vty_inventory_command(
            "data-source-preset-create --domain=runtime_traffic_accounting_source --name=shared:vnstat --definition-json={\"source\":\"vnstat\"}",
        )
        .unwrap(),
        VtyInventoryCommand::DataSourcePresetCreate {
            domain: "runtime_traffic_accounting_source".to_string(),
            name: "shared:vnstat".to_string(),
            scope: "shared".to_string(),
            owner_client_id: None,
            description: None,
            definition: serde_json::json!({"source": "vnstat"}),
        }
    );
    assert_eq!(
        parse_vty_inventory_command(
            "data-source-preset-clone --source-preset-id=11111111-1111-4111-8111-111111111111 --name=shared:copy --description copied",
        )
        .unwrap(),
        VtyInventoryCommand::DataSourcePresetClone {
            source_preset_id: "11111111-1111-4111-8111-111111111111".to_string(),
            name: "shared:copy".to_string(),
            scope: "shared".to_string(),
            owner_client_id: None,
            description: Some("copied".to_string()),
        }
    );
    assert_eq!(
        parse_vty_inventory_command(
            "data-source-preset-diff --preset-id=11111111-1111-4111-8111-111111111111 --definition-json={\"source\":\"vnstat\"}",
        )
        .unwrap(),
        VtyInventoryCommand::DataSourcePresetDiff {
            preset_id: "11111111-1111-4111-8111-111111111111".to_string(),
            description: None,
            clear_description: false,
            definition: serde_json::json!({"source": "vnstat"}),
        }
    );
    assert_eq!(
        parse_vty_inventory_command(
            "data-source-preset-test --preset-id=11111111-1111-4111-8111-111111111111 --definition-json={\"source\":\"interface_counters\"}",
        )
        .unwrap(),
        VtyInventoryCommand::DataSourcePresetTest {
            preset_id: "11111111-1111-4111-8111-111111111111".to_string(),
            definition: serde_json::json!({"source": "interface_counters"}),
        }
    );
    assert_eq!(
        parse_vty_inventory_command(
            "data-source-preset-update --preset-id 11111111-1111-4111-8111-111111111111 --clear-description --definition-json={\"source\":\"vnstat\"} --confirmed",
        )
        .unwrap(),
        VtyInventoryCommand::DataSourcePresetUpdate {
            preset_id: "11111111-1111-4111-8111-111111111111".to_string(),
            description: None,
            clear_description: true,
            definition: serde_json::json!({"source": "vnstat"}),
            confirmed: true,
        }
    );
    assert_eq!(
        parse_vty_inventory_command(
            "data-source-preset-assign --domain runtime_traffic_accounting_source --preset-id 11111111-1111-4111-8111-111111111111 --client edge-a --tag bgp --confirmed",
        )
        .unwrap(),
        VtyInventoryCommand::DataSourcePresetAssign {
            domain: "runtime_traffic_accounting_source".to_string(),
            preset_id: "11111111-1111-4111-8111-111111111111".to_string(),
            clients: vec!["edge-a".to_string()],
            tags: vec!["bgp".to_string()],
            confirmed: true,
        }
    );
    assert_eq!(
        parse_vty_inventory_command("data-source-hot-config --client-id=edge/a --format=json")
            .unwrap(),
        VtyInventoryCommand::DataSourceHotConfig {
            client_id: "edge/a".to_string(),
            format: "json".to_string(),
        }
    );
    assert_eq!(
        parse_vty_inventory_command(
            "data-source-status --client-id=edge/a --domain=runtime_traffic_accounting_source"
        )
        .unwrap(),
        VtyInventoryCommand::DataSourceStatus {
            client_id: Some("edge/a".to_string()),
            domain: Some("runtime_traffic_accounting_source".to_string()),
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
            "fleet-alert-notification-channel-upsert --name edge-audit --scope-kind tag --scope-value edge --min-severity warning --categories agent_status,network --operator-states open,escalated --delivery-kind audit_log --target audit:fleet --cooldown-secs 600 --notes page-edge --confirmed"
        )
        .unwrap(),
        VtyInventoryCommand::FleetAlertNotificationChannelUpsert {
            name: "edge-audit".to_string(),
            scope_kind: "tag".to_string(),
            scope_value: Some("edge".to_string()),
            min_severity: Some("warning".to_string()),
            categories: vec!["agent_status".to_string(), "network".to_string()],
            operator_states: vec!["open".to_string(), "escalated".to_string()],
            delivery_kind: "audit_log".to_string(),
            target: "audit:fleet".to_string(),
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
            confirmed: false,
        }
    );
    assert_eq!(
        parse_vty_inventory_command(
            "fleet-alert-notification-process --limit 25 --status failed --delivery-kind webhook --confirmed"
        )
        .unwrap(),
        VtyInventoryCommand::FleetAlertNotificationProcess {
            limit: 25,
            status: Some("failed".to_string()),
            delivery_kind: Some("webhook".to_string()),
            dry_run: false,
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
    assert!(parse_vty_inventory_command("data-source-preset-create --name x").is_err());
    assert!(parse_vty_inventory_command("data-source-preset-clone --name x").is_err());
    assert!(parse_vty_inventory_command("data-source-preset-diff --confirmed").is_err());
    assert!(parse_vty_inventory_command(
        "data-source-preset-update --description x --clear-description"
    )
    .is_err());
    assert!(parse_vty_inventory_command("data-source-preset-assign --domain x").is_err());
    assert!(parse_vty_inventory_command("unknown").is_err());
    assert!(gateway_sessions_path("gateway-sessions --limit=0").is_err());
    assert!(gateway_sessions_path("gateway-sessions extra").is_err());
    assert_eq!(
        telemetry_rollups_path(10, Some("edge/a"), Some(300)),
        "/api/v1/telemetry/rollups?limit=10&client_id=edge%2Fa&bucket_secs=300"
    );
    assert_eq!(
        telemetry_network_rates_path(10, Some("edge/a"), Some("eth/0"), Some(300)),
        "/api/v1/telemetry/network-rates?limit=10&client_id=edge%2Fa&interface=eth%2F0&bucket_secs=300"
    );
    assert_eq!(
        telemetry_tunnels_path(10, Some("edge/a"), Some("tun/0")),
        "/api/v1/telemetry/tunnels?limit=10&client_id=edge%2Fa&interface=tun%2F0"
    );
    assert_eq!(
        super::data_source_presets_path(Some("telemetry/source")),
        "/api/v1/data-source-presets?domain=telemetry%2Fsource"
    );
    assert_eq!(
        super::data_source_assignments_path(Some("edge/a"), Some("telemetry/source")),
        "/api/v1/data-source-assignments?client_id=edge%2Fa&domain=telemetry%2Fsource"
    );
    assert_eq!(
        super::data_source_status_path(Some("edge/a"), Some("telemetry/source")),
        "/api/v1/data-source-status?client_id=edge%2Fa&domain=telemetry%2Fsource"
    );
    assert_eq!(
        super::data_source_hot_config_path("edge/a"),
        "/api/v1/data-source-hot-config?client_id=edge%2Fa"
    );
    assert!(parse_vty_inventory_command("data-source-hot-config --format xml").is_err());
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
            Some("audit_log")
        ),
        "/api/v1/fleet-alert-notification-channels?limit=10&enabled=true&scope_kind=tag&scope_value=edge%2Fa&delivery_kind=audit_log"
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
    assert!(
        parse_vty_inventory_command("telemetry-network-rates --interface '' --bucket-secs 1")
            .is_err()
    );
    assert!(parse_vty_inventory_command("telemetry-tunnels --limit=0").is_err());
}
