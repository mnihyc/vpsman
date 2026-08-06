use std::path::PathBuf;

use super::{
    parse_vty_tunnel_allocate, parse_vty_tunnel_ospf_status_refresh, parse_vty_tunnel_plan,
    parse_vty_tunnel_plan_export, parse_vty_tunnel_plan_mutation, parse_vty_tunnel_status,
    vty_tunnel_plan_mutation_body,
};
use uuid::Uuid;
use vpsman_common::TunnelEndpointSide;

#[test]
fn parses_vty_tunnel_allocate() {
    let request = parse_vty_tunnel_allocate(&[
        "--ipv4-pool-cidr=10.255.40.0/24",
        "--ipv6-pool-cidr",
        "fd7a:115c:a1e0:40::/120",
        "--reserved=10.255.40.0,10.255.40.1",
        "--include-ipv6",
        "--include-ipv4=false",
    ])
    .unwrap();

    assert_eq!(request.ipv4_pool_cidr.as_deref(), Some("10.255.40.0/24"));
    assert_eq!(
        request.ipv6_pool_cidr.as_deref(),
        Some("fd7a:115c:a1e0:40::/120")
    );
    assert_eq!(
        request.reserved_addresses,
        vec!["10.255.40.0", "10.255.40.1"]
    );
    assert_eq!(request.include_ipv4, Some(false));
    assert_eq!(request.include_ipv6, Some(true));
}

#[test]
fn parses_vty_tunnel_plan_export() {
    let request = parse_vty_tunnel_plan_export(&[
        "--plan-id=00000000-0000-0000-0000-000000000001",
        "--output-file",
        "/tmp/plan.json",
    ])
    .unwrap();

    assert_eq!(
        request.plan_id,
        Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap()
    );
    assert_eq!(request.output_file, Some(PathBuf::from("/tmp/plan.json")));
}

#[test]
fn parses_explicit_plan_lifecycle_and_ospf_status_refresh() {
    let enabled = parse_vty_tunnel_plan_mutation(
        &[
            "--plan-id=00000000-0000-0000-0000-000000000001",
            "--expected-revision=3",
            "--confirmed",
        ],
        "tunnel-plan-enable",
    )
    .unwrap();
    let deleted = parse_vty_tunnel_plan_mutation(
        &[
            "--plan-id=00000000-0000-0000-0000-000000000001",
            "--expected-revision=4",
            "--confirmed",
        ],
        "tunnel-plan-delete",
    )
    .unwrap();
    let rotated = parse_vty_tunnel_plan_mutation(
        &[
            "--plan-id=00000000-0000-0000-0000-000000000001",
            "--expected-revision=5",
            "--confirmed",
        ],
        "tunnel-plan-rotate-credentials",
    )
    .unwrap();
    let status = parse_vty_tunnel_ospf_status_refresh(&[
        "--plan-id",
        "00000000-0000-0000-0000-000000000001",
    ])
    .unwrap();

    assert!(enabled.confirmed);
    assert_eq!(enabled.expected_revision, 3);
    assert!(deleted.confirmed);
    assert_eq!(deleted.expected_revision, 4);
    assert!(rotated.confirmed);
    assert_eq!(rotated.expected_revision, 5);
    assert_eq!(enabled.plan_id, deleted.plan_id);
    assert_eq!(enabled.plan_id, rotated.plan_id);
    assert_eq!(enabled.plan_id, status.plan_id);
    assert!(parse_vty_tunnel_plan_mutation(
        &[
            "--plan-id=00000000-0000-0000-0000-000000000001",
            "--expected-revision=3",
        ],
        "tunnel-plan-disable",
    )
    .is_err());
}

#[test]
fn parses_vty_tunnel_status_without_confirmation() {
    let request = parse_vty_tunnel_status(&[
        "--plan-id=00000000-0000-0000-0000-000000000001",
        "--side=right",
        "--max-timeout=45",
    ])
    .unwrap();

    assert_eq!(
        request.plan_id,
        Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap()
    );
    assert_eq!(request.side, TunnelEndpointSide::Right);
    assert_eq!(request.max_timeout_secs, 45);
    assert!(parse_vty_tunnel_status(&[
        "--plan-id=00000000-0000-0000-0000-000000000001",
        "--side=right",
        "--privilege-ttl=75",
    ])
    .is_err());
}

#[test]
fn vty_plan_update_preserves_lifecycle_without_explicit_enabled_flag() {
    let request = parse_vty_tunnel_plan(&[
        "--save",
        "--confirmed",
        "--update-plan-id=00000000-0000-4000-8000-000000000001",
        "--expected-revision=7",
        "--name=edge",
        "--interface=tun0",
        "--kind=gre",
        "--left-client=left",
        "--right-client=right",
        "--left-remote-underlay=198.51.100.10",
        "--right-remote-underlay=203.0.113.20",
        "--left-tunnel-ipv4-cidr=10.255.0.0/31",
        "--right-tunnel-ipv4-cidr=10.255.0.1/31",
        "--bandwidth-mbps=100",
    ])
    .unwrap();
    let body = vty_tunnel_plan_mutation_body(&request).unwrap();

    assert_eq!(body["expected_revision"], 7);
    assert_eq!(body["confirmed"], true);
    assert!(body.get("enabled").is_none());
}
