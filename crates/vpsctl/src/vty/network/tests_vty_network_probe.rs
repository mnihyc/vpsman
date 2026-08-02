use super::parse_vty_tunnel_probe;
use vpsman_common::TunnelEndpointSide;

#[test]
fn parses_vty_tunnel_probe_with_bounds() {
    let request = parse_vty_tunnel_probe(&[
        "--plan-id=00000000-0000-0000-0000-000000000001",
        "--side",
        "right",
        "--count=5",
        "--interval-ms",
        "750",
        "--max-timeout=120",
        "--privilege-ttl",
        "90",
    ])
    .unwrap();

    assert_eq!(
        request.plan_id,
        uuid::Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap()
    );
    assert_eq!(request.side, TunnelEndpointSide::Right);
    assert_eq!(request.count, 5);
    assert_eq!(request.interval_ms, 750);
    assert_eq!(request.max_timeout_secs, 120);
    assert_eq!(request.privilege_ttl_secs, 90);
}

#[test]
fn rejects_vty_tunnel_probe_bad_bounds_or_side() {
    assert!(parse_vty_tunnel_probe(&[
        "--plan-id=00000000-0000-0000-0000-000000000001",
        "--side=left",
        "--count=0",
    ])
    .is_err());
    assert!(parse_vty_tunnel_probe(&[
        "--plan-id=00000000-0000-0000-0000-000000000001",
        "--side=left",
        "--interval-ms=199",
    ])
    .is_err());
    assert!(parse_vty_tunnel_probe(&[
        "--plan-id=00000000-0000-0000-0000-000000000001",
        "--side=middle",
    ])
    .is_err());
    assert!(parse_vty_tunnel_probe(&["--plan-id=00000000-0000-0000-0000-000000000001",]).is_err());
}
