use super::*;
use vpsman_common::{
    plan_tunnel, RuntimeTunnelControl, RuntimeTunnelManager, TunnelAddressPair, TunnelKind,
    TunnelPlanInput,
};

fn speed_test_plan() -> TunnelPlan {
    plan_tunnel(&TunnelPlanInput {
        name: "speed-link".to_string(),
        interface_name: "tun-speed".to_string(),
        kind: TunnelKind::Gre,
        runtime_control: RuntimeTunnelControl {
            manager: RuntimeTunnelManager::AgentBuiltin,
            ..RuntimeTunnelControl::default()
        },
        runtime_topology: Default::default(),
        left_client_id: "left-vps".to_string(),
        right_client_id: "right-vps".to_string(),
        left_remote_underlay: "198.51.100.10".to_string(),
        right_remote_underlay: "203.0.113.20".to_string(),
        left_local_underlay: None,
        right_local_underlay: None,
        address_pool_cidr: "10.255.0.0/24".to_string(),
        reserved_addresses: Vec::new(),
        ipv4_tunnel: Some(TunnelAddressPair {
            left: "10.255.0.0".to_string(),
            right: "10.255.0.1".to_string(),
            prefix_len: 31,
        }),
        ipv6_address_pool_cidr: None,
        ipv6_tunnel: None,
        latency_primary_family: Default::default(),
        bandwidth_mbps: 100,
        left_mtu: Some(1476),
        right_mtu: Some(1476),
        ospf: None,
    })
    .unwrap()
}

#[test]
fn speed_test_nonce_is_job_and_payload_bound() {
    let job_id = uuid::Uuid::parse_str("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa").unwrap();
    let first = speed_test_nonce_hex(job_id, "payload-a");
    let second = speed_test_nonce_hex(job_id, "payload-b");
    assert_eq!(first.len(), 64);
    assert_ne!(first, second);
    assert_eq!(first, speed_test_nonce_hex(job_id, "payload-a"));
}

#[test]
fn socket_address_requires_an_explicit_ip() {
    assert_eq!(
        socket_addr("10.255.0.1", 42000).unwrap(),
        "10.255.0.1:42000".parse().unwrap()
    );
    assert!(socket_addr("example.invalid", 42000).is_err());
}

#[test]
fn zero_byte_limit_produces_unlimited_full_chunks() {
    assert_eq!(speed_chunk_limit(0, 0), SPEED_CHUNK_BYTES);
    assert_eq!(speed_chunk_limit(0, u64::MAX), SPEED_CHUNK_BYTES);
    assert_eq!(speed_chunk_limit(20_000, 10_000), 10_000);
    assert_eq!(speed_chunk_limit(20_000, 20_000), 0);
}

#[test]
fn finite_rate_budget_waits_until_the_accumulated_bytes_are_allowed() {
    assert_eq!(rate_budget_delay(Duration::ZERO, 16_384, 0), None);
    assert_eq!(
        rate_budget_delay(Duration::from_millis(100), 16_384, 64),
        Some(Duration::from_millis(100))
    );
    assert_eq!(rate_budget_delay(Duration::from_secs(3), 16_384, 64), None);
}

#[test]
fn throughput_intervals_include_the_final_partial_interval() {
    let mut collector = SpeedIntervalCollector::default();
    collector.observe(Duration::from_millis(999), 100_000);
    assert!(collector.intervals.is_empty());

    collector.observe(Duration::from_millis(1_000), 125_000);
    let intervals = collector.finish(Duration::from_millis(1_500), 250_000);

    assert_eq!(intervals.len(), 2);
    assert_eq!(intervals[0].start_ms, 0.0);
    assert_eq!(intervals[0].end_ms, 1_000.0);
    assert_eq!(intervals[0].bytes, 125_000);
    assert!((intervals[0].throughput_mbps - 1.0).abs() < f64::EPSILON);
    assert_eq!(intervals[1].start_ms, 1_000.0);
    assert_eq!(intervals[1].end_ms, 1_500.0);
    assert_eq!(intervals[1].bytes, 125_000);
    assert!((intervals[1].throughput_mbps - 2.0).abs() < f64::EPSILON);
}

#[test]
fn throughput_intervals_ignore_shutdown_slack_but_preserve_a_full_stall() {
    let mut shutdown_slack = SpeedIntervalCollector::default();
    shutdown_slack.observe(Duration::from_secs(1), 125_000);
    let intervals = shutdown_slack.finish(Duration::from_millis(1_010), 125_000);
    assert_eq!(intervals.len(), 1);

    let mut stalled = SpeedIntervalCollector::default();
    stalled.observe(Duration::from_secs(1), 125_000);
    let intervals = stalled.finish(Duration::from_secs(2), 125_000);
    assert_eq!(intervals.len(), 2);
    assert_eq!(intervals[1].bytes, 0);
    assert_eq!(intervals[1].throughput_mbps, 0.0);
}

#[test]
fn throughput_intervals_preserve_full_mid_run_stalls_as_one_second_buckets() {
    let mut collector = SpeedIntervalCollector::default();
    collector.observe(Duration::from_secs(1), 125_000);
    collector.observe(Duration::from_secs(4), 250_000);
    let intervals = collector.finish(Duration::from_secs(4), 250_000);

    assert_eq!(intervals.len(), 4);
    assert!(intervals
        .iter()
        .all(|interval| interval.end_ms - interval.start_ms == 1_000.0));
    assert_eq!(
        intervals
            .iter()
            .map(|interval| interval.bytes)
            .collect::<Vec<_>>(),
        vec![125_000, 0, 0, 125_000]
    );
    assert_eq!(intervals[1].throughput_mbps, 0.0);
    assert_eq!(intervals[2].throughput_mbps, 0.0);
}

#[test]
fn throughput_intervals_are_bounded_to_the_command_duration_limit() {
    let mut collector = SpeedIntervalCollector::default();
    for second in 1..=SPEED_INTERVAL_MAX_SAMPLES + 5 {
        collector.observe(Duration::from_secs(second as u64), second as u64 * 125_000);
    }
    let intervals = collector.finish(Duration::from_secs(40), 5_000_000);
    assert_eq!(intervals.len(), SPEED_INTERVAL_MAX_SAMPLES);
}

#[test]
fn speed_direction_identifies_the_payload_sender_and_receiver() {
    assert_eq!(
        speed_test_direction(TunnelEndpointSide::Left, "left-vps", "right-vps"),
        ("right_to_left", "right-vps", "left-vps")
    );
    assert_eq!(
        speed_test_direction(TunnelEndpointSide::Right, "left-vps", "right-vps"),
        ("left_to_right", "left-vps", "right-vps")
    );
}

#[test]
fn empty_or_unmeasurable_transfers_do_not_report_zero_throughput() {
    assert_eq!(measured_throughput_mbps(0, Duration::from_secs(1)), None);
    assert_eq!(measured_throughput_mbps(1_000, Duration::ZERO), None);
    assert_eq!(
        measured_throughput_mbps(125_000, Duration::from_secs(1)),
        Some(1.0)
    );
}

#[test]
fn transfer_completion_requires_duration_or_the_finite_byte_cap() {
    let duration = Duration::from_secs(10);
    assert!(!speed_transfer_completed(
        duration,
        NETWORK_SPEED_TEST_UNLIMITED_MAX_BYTES,
        125_000,
        Duration::from_secs(2),
    ));
    assert!(speed_transfer_completed(
        duration,
        NETWORK_SPEED_TEST_UNLIMITED_MAX_BYTES,
        125_000,
        Duration::from_millis(9_500),
    ));
    assert!(speed_transfer_completed(
        duration,
        125_000,
        125_000,
        Duration::from_millis(50),
    ));
    assert!(!speed_transfer_completed(
        duration,
        125_000,
        124_999,
        Duration::from_millis(50),
    ));
}

#[test]
fn status_evidence_keeps_direction_and_bounded_interval_statistics_explicit() {
    let plan = speed_test_plan();
    let output = status_output(
        NetworkSpeedRoleInput {
            job_id: uuid::Uuid::parse_str("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa").unwrap(),
            command_payload_hash: "payload-hash",
            plan: &plan,
            client_id: "right-vps",
            peer_client_id: "left-vps",
            role: "server",
            server_side: TunnelEndpointSide::Right,
            server_address: "10.255.0.1",
            peer_tunnel_address: "10.255.0.0",
            port: 5201,
            duration: Duration::from_secs(2),
            max_bytes: 0,
            rate_limit_kbps: 0,
            connect_timeout: Duration::from_secs(5),
        },
        None,
        375_000,
        Duration::from_secs(2),
        vec![
            SpeedThroughputInterval {
                start_ms: 0.0,
                end_ms: 1_000.0,
                bytes: 125_000,
                throughput_mbps: 1.0,
            },
            SpeedThroughputInterval {
                start_ms: 1_000.0,
                end_ms: 2_000.0,
                bytes: 250_000,
                throughput_mbps: 2.0,
            },
        ],
        true,
        None,
    );
    let status: serde_json::Value = serde_json::from_slice(&output.data).unwrap();

    assert_eq!(status["direction"], "left_to_right");
    assert_eq!(status["sender_client_id"], "left-vps");
    assert_eq!(status["receiver_client_id"], "right-vps");
    assert_eq!(status["throughput_min_mbps"], 1.0);
    assert_eq!(status["throughput_mbps"], 1.5);
    assert_eq!(status["throughput_max_mbps"], 2.0);
    assert_eq!(status["throughput_intervals"].as_array().unwrap().len(), 2);
    assert_eq!(output.exit_code, Some(0));
}

#[test]
fn incomplete_transfer_fails_but_retains_measured_evidence() {
    let plan = speed_test_plan();
    let output = status_output(
        NetworkSpeedRoleInput {
            job_id: uuid::Uuid::parse_str("bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb").unwrap(),
            command_payload_hash: "payload-hash",
            plan: &plan,
            client_id: "right-vps",
            peer_client_id: "left-vps",
            role: "server",
            server_side: TunnelEndpointSide::Right,
            server_address: "10.255.0.1",
            peer_tunnel_address: "10.255.0.0",
            port: 5201,
            duration: Duration::from_secs(10),
            max_bytes: NETWORK_SPEED_TEST_UNLIMITED_MAX_BYTES,
            rate_limit_kbps: NETWORK_SPEED_TEST_UNLIMITED_RATE_LIMIT_KBPS,
            connect_timeout: Duration::from_secs(5),
        },
        None,
        125_000,
        Duration::from_secs(1),
        vec![SpeedThroughputInterval {
            start_ms: 0.0,
            end_ms: 1_000.0,
            bytes: 125_000,
            throughput_mbps: 1.0,
        }],
        false,
        None,
    );
    let status: serde_json::Value = serde_json::from_slice(&output.data).unwrap();

    assert_eq!(status["success"], false);
    assert_eq!(status["reason"], "transfer_incomplete");
    assert_eq!(status["throughput_mbps"], 1.0);
    assert_eq!(status["throughput_min_mbps"], 1.0);
    assert_eq!(status["throughput_max_mbps"], 1.0);
    assert_eq!(status["throughput_intervals"].as_array().unwrap().len(), 1);
    assert_eq!(output.exit_code, Some(1));
}
