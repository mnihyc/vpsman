use super::*;
use vpsman_common::{
    plan_tunnel, RuntimeTunnelControl, TunnelAddressFamily, TunnelAddressPair,
    TunnelEndpointBuiltinCredentials, TunnelPlanInput,
};

fn plan(kind: TunnelKind) -> TunnelPlan {
    plan_tunnel(&TunnelPlanInput {
        name: "test-plan".to_string(),
        interface_name: "tun-test".to_string(),
        kind,
        runtime_control: RuntimeTunnelControl::default(),
        runtime_topology: Default::default(),
        left_client_id: "v-1".to_string(),
        right_client_id: "v-2".to_string(),
        left_remote_underlay: "192.0.2.2".to_string(),
        left_local_underlay: None,
        right_remote_underlay: "192.0.2.1".to_string(),
        right_local_underlay: None,
        address_pool_cidr: "10.0.0.0/31".to_string(),
        reserved_addresses: Vec::new(),
        ipv4_tunnel: Some(TunnelAddressPair {
            left: "10.0.0.0".to_string(),
            right: "10.0.0.1".to_string(),
            prefix_len: 31,
        }),
        ipv6_address_pool_cidr: None,
        ipv6_tunnel: None,
        latency_primary_family: TunnelAddressFamily::Ipv4,
        bandwidth_mbps: 100,
        left_mtu: vpsman_common::default_tunnel_mtu(kind),
        right_mtu: vpsman_common::default_tunnel_mtu(kind),
        ospf: None,
    })
    .unwrap()
}

#[test]
fn wireguard_generation_is_complete_and_endpoint_scoped() {
    let credentials =
        generate_tunnel_builtin_credentials(Uuid::new_v4(), &plan(TunnelKind::Wireguard), 1)
            .unwrap()
            .unwrap();
    let left = credentials.endpoint(TunnelEndpointSide::Left);
    let right = credentials.endpoint(TunnelEndpointSide::Right);
    assert_ne!(left, right);
    assert_eq!(credentials.generation(), 1);
}

#[test]
fn openvpn_certificates_are_distinct_and_endpoint_trust_is_cross_projected() {
    let credentials =
        generate_tunnel_builtin_credentials(Uuid::new_v4(), &plan(TunnelKind::Openvpn), 7)
            .unwrap()
            .unwrap();
    let TunnelBuiltinCredentials::Openvpn { left, right, .. } = credentials else {
        panic!("expected OpenVPN credentials");
    };
    assert_ne!(left.certificate_pem, right.certificate_pem);
    assert_ne!(left.issuer_certificate_pem, right.issuer_certificate_pem);
    assert_eq!(left.certificate_sha256_fingerprint.len(), 95);
    assert_eq!(right.certificate_sha256_fingerprint.len(), 95);

    let left_endpoint = TunnelBuiltinCredentials::Openvpn {
        generation: 7,
        left: left.clone(),
        right: right.clone(),
    }
    .endpoint(TunnelEndpointSide::Left);
    let TunnelEndpointBuiltinCredentials::Openvpn {
        peer_issuer_certificate_pem,
        ..
    } = left_endpoint
    else {
        panic!("expected endpoint OpenVPN credentials");
    };
    assert_eq!(peer_issuer_certificate_pem, right.issuer_certificate_pem);
}

#[test]
fn endpoint_change_rotates_only_the_affected_identity() {
    let plan_id = Uuid::new_v4();
    let previous_plan = plan(TunnelKind::Wireguard);
    let previous = generate_tunnel_builtin_credentials(plan_id, &previous_plan, 1)
        .unwrap()
        .unwrap();
    let mut next_plan = previous_plan.clone();
    next_plan.left_client_id = "v-3".to_string();
    let next = reconcile_tunnel_builtin_credentials(
        plan_id,
        Some(&previous_plan),
        Some(&previous),
        &next_plan,
    )
    .unwrap()
    .unwrap();
    let (
        TunnelBuiltinCredentials::Wireguard {
            left: previous_left,
            right: previous_right,
            ..
        },
        TunnelBuiltinCredentials::Wireguard {
            left: next_left,
            right: next_right,
            generation,
        },
    ) = (previous, next)
    else {
        panic!("expected WireGuard credentials");
    };
    assert_ne!(previous_left, next_left);
    assert_eq!(previous_right, next_right);
    assert_eq!(generation, 2);
}

#[test]
fn unrelated_plan_edits_preserve_credentials_and_generation() {
    let plan_id = Uuid::new_v4();
    let previous_plan = plan(TunnelKind::Wireguard);
    let previous = generate_tunnel_builtin_credentials(plan_id, &previous_plan, 1)
        .unwrap()
        .unwrap();
    let mut next_plan = previous_plan.clone();
    next_plan.bandwidth_mbps = 900;
    next_plan.left_mtu = Some(1400);

    let next = reconcile_tunnel_builtin_credentials(
        plan_id,
        Some(&previous_plan),
        Some(&previous),
        &next_plan,
    )
    .unwrap()
    .unwrap();

    assert_eq!(next, previous);
    assert_eq!(next.generation(), 1);
}
