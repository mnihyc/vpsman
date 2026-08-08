use super::*;

#[test]
fn parses_probe_and_speed_status_observations() {
    let job_id = Uuid::new_v4();
    let probe = CommandOutput {
        job_id,
        stream: OutputStream::Status,
        data: serde_json::to_vec(&serde_json::json!({
            "type": "tunnel_reachability",
            "plan": "edge",
            "interface": "tun0",
            "peer_client_id": "right",
            "target": "10.0.0.1",
            "parsed": {
                "healthy": true,
                "latency_avg_ms": 12.5,
                "packet_loss_ratio": 0.01
            }
        }))
        .unwrap(),
        exit_code: Some(0),
        done: true,
    };
    let speed = CommandOutput {
        job_id,
        stream: OutputStream::Status,
        data: serde_json::to_vec(&serde_json::json!({
            "type": "network_speed_test",
            "role": "client",
            "plan": "edge",
            "interface": "tun0",
            "peer_client_id": "left",
            "server_address": "10.0.0.0",
            "port": 5201,
            "success": true,
            "bytes": 1048576,
            "throughput_mbps": 33.3
        }))
        .unwrap(),
        exit_code: Some(0),
        done: true,
    };

    let parsed_probe = parse_network_observation(job_id, "left", 0, &probe, "1").unwrap();
    let parsed_speed = parse_network_observation(job_id, "right", 1, &speed, "1").unwrap();

    assert_eq!(parsed_probe.kind, "tunnel_reachability");
    assert_eq!(parsed_probe.latency_avg_ms, Some(12.5));
    assert_eq!(parsed_probe.packet_loss_ratio, Some(0.01));
    assert_eq!(parsed_probe.healthy, Some(true));
    assert_eq!(parsed_speed.kind, "network_speed_test");
    assert_eq!(parsed_speed.role.as_deref(), Some("client"));
    assert_eq!(parsed_speed.target.as_deref(), Some("10.0.0.0:5201"));
    assert_eq!(parsed_speed.bytes, Some(1_048_576));
    assert_eq!(parsed_speed.throughput_mbps, Some(33.3));
}

#[test]
fn legacy_network_probe_status_is_not_a_reachability_observation() {
    let job_id = Uuid::new_v4();
    let output = CommandOutput {
        job_id,
        stream: OutputStream::Status,
        data: serde_json::to_vec(&serde_json::json!({
            "type": "network_probe",
            "plan": "edge",
            "interface": "tun0",
            "peer_client_id": "right",
            "target": "10.0.0.1",
            "parsed": {
                "healthy": true,
                "latency_avg_ms": 12.5,
                "packet_loss_ratio": 0.0
            }
        }))
        .unwrap(),
        exit_code: Some(0),
        done: true,
    };

    assert!(parse_network_observation(job_id, "left", 0, &output, "1").is_none());
}

#[test]
fn parses_network_status_runtime_summary_and_adapter_evidence() {
    let job_id = Uuid::new_v4();
    let status = CommandOutput {
        job_id,
        stream: OutputStream::Status,
        data: serde_json::to_vec(&serde_json::json!({
            "type": "network_status",
            "plan": "external-edge",
            "interface": "ovpn42",
            "peer_client_id": "right",
            "runtime": {
                "summary": {
                    "manager": "custom_adapter",
                    "status": "adapter_unhealthy",
                    "healthy": false,
                    "drift": false,
                    "reasons": ["adapter_status_failed"]
                },
                "adapter": {
                    "configured": true,
                    "success": false,
                    "exit_code": 7
                }
            }
        }))
        .unwrap(),
        exit_code: Some(0),
        done: true,
    };

    let parsed = parse_network_observation(job_id, "left", 3, &status, "1").unwrap();

    assert_eq!(parsed.kind, "network_status");
    assert_eq!(parsed.plan_name.as_deref(), Some("external-edge"));
    assert_eq!(parsed.interface_name.as_deref(), Some("ovpn42"));
    assert_eq!(parsed.healthy, Some(false));
    assert_eq!(
        parsed.metadata["runtime"]["summary"]["status"],
        "adapter_unhealthy"
    );
}

#[test]
fn fair_limit_keeps_every_plan_visible_when_one_plan_is_noisy() {
    fn observation(plan_id: Uuid, endpoint: &str, observed_at: i64) -> NetworkObservationView {
        NetworkObservationView {
            id: Uuid::new_v4(),
            job_id: None,
            client_id: format!("{plan_id}-{endpoint}"),
            seq: None,
            kind: "tunnel_reachability".to_string(),
            source: "automatic".to_string(),
            role: Some("endpoint".to_string()),
            plan_id: Some(plan_id),
            topology_identity_hash: Some(plan_id.simple().to_string()),
            plan_name: Some(format!("plan-{plan_id}")),
            interface_name: Some("tun0".to_string()),
            peer_client_id: Some("peer".to_string()),
            target: Some("10.0.0.2".to_string()),
            endpoint_side: Some(endpoint.to_string()),
            address_family: Some("ipv4".to_string()),
            stale_after_secs: Some(180),
            healthy: Some(true),
            transmitted: Some(3),
            received: Some(3),
            latency_min_ms: Some(1.0),
            latency_avg_ms: Some(2.0),
            latency_max_ms: Some(3.0),
            latency_mdev_ms: Some(0.1),
            packet_loss_ratio: Some(0.0),
            reason: None,
            throughput_mbps: None,
            bytes: None,
            metadata: serde_json::json!({}),
            observed_at: observed_at.to_string(),
            received_at: observed_at.to_string(),
        }
    }

    let plan_ids = (0..25).map(|_| Uuid::new_v4()).collect::<Vec<_>>();
    let mut rows = Vec::new();
    for plan_id in &plan_ids {
        rows.push(observation(*plan_id, "left", 1_000));
        rows.push(observation(*plan_id, "right", 999));
    }
    for offset in 0..100 {
        rows.push(observation(plan_ids[0], "left", 2_000 + offset));
    }
    rows.sort_by(compare_network_observations_desc);

    let limited = limit_observations_fairly(rows, 2, 10_000);
    let represented = limited
        .iter()
        .filter_map(|row| row.plan_id)
        .collect::<std::collections::HashSet<_>>();

    assert_eq!(represented.len(), plan_ids.len());
    assert!(plan_ids.iter().all(|plan_id| represented.contains(plan_id)));
    assert!(
        limited
            .iter()
            .filter(|row| row.plan_id == Some(plan_ids[0])
                && row.endpoint_side.as_deref() == Some("left"))
            .count()
            <= 2
    );
}
