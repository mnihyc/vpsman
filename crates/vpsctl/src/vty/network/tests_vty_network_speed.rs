use super::parse_vty_tunnel_speed_test;
use vpsman_common::TunnelEndpointSide;

#[test]
fn parses_vty_tunnel_speed_test_with_bounds() {
    let request = parse_vty_tunnel_speed_test(&[
        "--plan-id=00000000-0000-0000-0000-000000000001",
        "--server-side",
        "right",
        "--duration-secs=5",
        "--max-bytes",
        "1048576",
        "--rate-limit-kbps=10000",
        "--port",
        "55201",
        "--connect-timeout-ms=2500",
        "--max-timeout=120",
        "--privilege-ttl",
        "90",
        "--confirmed",
    ])
    .unwrap();

    assert_eq!(
        request.plan_id,
        uuid::Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap()
    );
    assert_eq!(request.server_side, TunnelEndpointSide::Right);
    assert_eq!(request.duration_secs, 5);
    assert_eq!(request.max_bytes, 1_048_576);
    assert_eq!(request.rate_limit_kbps, 10_000);
    assert_eq!(request.port, 55_201);
    assert_eq!(request.connect_timeout_ms, 2500);
    assert_eq!(request.max_timeout_secs, 120);
    assert_eq!(request.privilege_ttl_secs, 90);
    assert!(request.confirmed);
}

#[test]
fn rejects_vty_tunnel_speed_test_bad_bounds_or_side() {
    assert!(parse_vty_tunnel_speed_test(&[
        "--plan-id=00000000-0000-0000-0000-000000000001",
        "--server-side=left",
        "--duration-secs=0",
    ])
    .is_err());
    assert!(parse_vty_tunnel_speed_test(&[
        "--plan-id=00000000-0000-0000-0000-000000000001",
        "--server-side=left",
        "--max-bytes=1",
    ])
    .is_err());
    assert!(parse_vty_tunnel_speed_test(&[
        "--plan-id=00000000-0000-0000-0000-000000000001",
        "--server-side=middle",
        "--confirmed",
    ])
    .is_err());
    assert!(
        parse_vty_tunnel_speed_test(&["--plan-id=00000000-0000-0000-0000-000000000001",]).is_err()
    );
    assert!(parse_vty_tunnel_speed_test(&[
        "--plan-id=00000000-0000-0000-0000-000000000001",
        "--server-side=left",
    ])
    .is_err());
}
