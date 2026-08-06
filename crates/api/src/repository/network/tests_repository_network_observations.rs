use super::*;

#[test]
fn parses_probe_and_speed_status_observations() {
    let job_id = Uuid::new_v4();
    let probe = CommandOutput {
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

    assert_eq!(parsed_probe.kind, "network_probe");
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
