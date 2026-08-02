use super::*;

#[test]
fn fleet_alert_policy_rejects_invalid_thresholds() {
    assert!(FleetAlertPolicy::new(0.10, 0.20, 0.20, 0.10, 2.0, 4.0).is_err());
    assert!(FleetAlertPolicy::new(0.20, 0.10, 0.10, 0.20, 2.0, 4.0).is_err());
    assert!(FleetAlertPolicy::new(0.20, 0.10, 0.20, 0.10, 4.0, 2.0).is_err());
    assert!(FleetAlertPolicy::new(0.20, 0.10, 0.20, 0.10, 2.0, 4.0).is_ok());
}

#[test]
fn resource_alerts_use_configurable_policy_thresholds() {
    let policy = FleetAlertPolicy::new(0.50, 0.25, 0.40, 0.15, 1.0, 2.5).unwrap();
    let mut rollups = HashMap::new();
    rollups.insert(
        "edge-a".to_string(),
        TelemetryRollupView {
            client_id: "edge-a".to_string(),
            bucket_start: "100".to_string(),
            bucket_secs: 60,
            sample_count: 3,
            cpu_usage_sample_count: 0,
            cpu_usage_avg: None,
            cpu_usage_max: None,
            cpu_cores_max: 0,
            cpu_load_1_avg: 1.5,
            cpu_load_1_max: 2.6,
            cpu_load_5_avg: 1.5,
            cpu_load_5_max: 2.6,
            cpu_load_15_avg: 1.5,
            cpu_load_15_max: 2.6,
            memory_total_bytes_max: 1000,
            memory_available_bytes_avg: 400,
            memory_available_bytes_min: 300,
            disk_total_bytes_max: 2000,
            disk_available_bytes_avg: 500,
            disk_available_bytes_min: 200,
            network_rx_bytes_max: 0,
            network_tx_bytes_max: 0,
            connections_sample_count: 0,
            tcp_sockets_latest: None,
            udp_sockets_latest: None,
            connections_observed_at: None,
            latest_observed_at: "120".to_string(),
            updated_at: "121".to_string(),
        },
    );

    let mut alerts = Vec::new();
    append_resource_alerts(&mut alerts, &rollups, &policy).unwrap();

    let cpu = find_status(&alerts, "cpu_load_high");
    assert_eq!(cpu.severity, "critical");
    assert_eq!(
        cpu.evidence["threshold"].as_f64().unwrap(),
        policy.cpu_load_critical
    );

    let memory = find_status(&alerts, "memory_low");
    assert_eq!(memory.severity, "warning");
    assert_eq!(
        memory.evidence["warning_threshold"].as_f64().unwrap(),
        policy.memory_available_warning_ratio
    );

    let disk = find_status(&alerts, "disk_low");
    assert_eq!(disk.severity, "critical");
    assert_eq!(
        disk.evidence["critical_threshold"].as_f64().unwrap(),
        policy.disk_available_critical_ratio
    );
}

fn find_status<'a>(alerts: &'a [FleetAlertView], status: &str) -> &'a FleetAlertView {
    alerts
        .iter()
        .find(|alert| alert.status == status)
        .unwrap_or_else(|| panic!("missing {status} in {alerts:#?}"))
}
