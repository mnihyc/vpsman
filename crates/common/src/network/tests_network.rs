use super::*;

const LEFT_RUNTIME_ADAPTER: &str = "11111111-1111-4111-8111-111111111111";
const RIGHT_RUNTIME_ADAPTER: &str = "22222222-2222-4222-8222-222222222222";
const LEFT_ROUTING_ADAPTER: &str = "33333333-3333-4333-8333-333333333333";
const RIGHT_ROUTING_ADAPTER: &str = "44444444-4444-4444-8444-444444444444";

fn ipv4_pair(left: &str, right: &str) -> TunnelAddressPair {
    TunnelAddressPair {
        left: left.to_string(),
        right: right.to_string(),
        prefix_len: 31,
    }
}

fn plan_input(kind: TunnelKind, manager: RuntimeTunnelManager) -> TunnelPlanInput {
    let runtime_control = RuntimeTunnelControl {
        manager,
        left_adapter_definition_id: (manager == RuntimeTunnelManager::CustomAdapter)
            .then(|| LEFT_RUNTIME_ADAPTER.to_string()),
        right_adapter_definition_id: (manager == RuntimeTunnelManager::CustomAdapter)
            .then(|| RIGHT_RUNTIME_ADAPTER.to_string()),
        ..RuntimeTunnelControl::default()
    };
    TunnelPlanInput {
        name: "edge-a-edge-b".to_string(),
        interface_name: "tunab".to_string(),
        kind,
        runtime_control,
        runtime_topology: RuntimeTunnelTopologyIntent::default(),
        left_client_id: "edge-a".to_string(),
        right_client_id: "edge-b".to_string(),
        left_remote_underlay: "198.51.100.10".to_string(),
        right_remote_underlay: "203.0.113.20".to_string(),
        left_local_underlay: None,
        right_local_underlay: None,
        address_pool_cidr: "10.255.0.0/24".to_string(),
        reserved_addresses: Vec::new(),
        ipv4_tunnel: Some(ipv4_pair("10.255.0.0", "10.255.0.1")),
        ipv6_address_pool_cidr: None,
        ipv6_tunnel: None,
        latency_primary_family: TunnelAddressFamily::Ipv4,
        bandwidth_mbps: 1234,
        left_mtu: (manager == RuntimeTunnelManager::AgentBuiltin)
            .then(|| default_tunnel_mtu(kind))
            .flatten(),
        right_mtu: (manager == RuntimeTunnelManager::AgentBuiltin)
            .then(|| default_tunnel_mtu(kind))
            .flatten(),
        ospf: None,
    }
}

#[test]
fn tunnel_mtu_defaults_are_kind_aware_1500_underlay_baselines() {
    assert_eq!(default_tunnel_mtu(TunnelKind::Gre), Some(1476));
    assert_eq!(default_tunnel_mtu(TunnelKind::Ipip), Some(1480));
    assert_eq!(default_tunnel_mtu(TunnelKind::Sit), Some(1480));
    assert_eq!(default_tunnel_mtu(TunnelKind::Fou), Some(1472));
    assert_eq!(default_tunnel_mtu(TunnelKind::Wireguard), Some(1420));
    assert_eq!(default_tunnel_mtu(TunnelKind::Openvpn), Some(1500));
    assert_eq!(default_tunnel_mtu(TunnelKind::TunTap), None);
    assert_eq!(default_tunnel_mtu(TunnelKind::Custom), None);
}

#[test]
fn openvpn_listener_bind_matches_the_initiators_destination_family() {
    let mut input = plan_input(TunnelKind::Openvpn, RuntimeTunnelManager::AgentBuiltin);
    input.runtime_control.openvpn.listener_side = TunnelEndpointSide::Left;
    input.left_remote_underlay = "198.51.100.20".to_string();
    input.right_remote_underlay = "2001:db8::10".to_string();
    input.left_local_underlay = Some("2001:db8::10".to_string());
    input.right_local_underlay = Some("2001:db8::20".to_string());
    assert!(plan_tunnel(&input).is_ok());

    input.left_local_underlay = Some("198.51.100.10".to_string());
    assert_eq!(
        plan_tunnel(&input).unwrap_err(),
        NetworkPlanError::InvalidUnderlayAddress
    );

    input.left_local_underlay = Some("  ".to_string());
    input.right_local_underlay = Some(String::new());
    assert!(plan_tunnel(&input).is_ok());
}

fn ospf_config() -> TunnelOspfConfig {
    TunnelOspfConfig {
        mode: OspfControlMode::Reviewed,
        planned_latency_ms: 20.0,
        planned_packet_loss_ratio: 0.01,
        preference: 1.0,
        policy: OspfCostPolicy::default(),
        min_cost_delta: 5,
        healthy_windows: 2,
        left_adapter_definition_id: Some(LEFT_ROUTING_ADAPTER.to_string()),
        right_adapter_definition_id: Some(RIGHT_ROUTING_ADAPTER.to_string()),
    }
}

#[test]
fn routing_cost_privilege_payload_freezes_both_endpoint_snapshots() {
    let payload = routing_cost_update_privilege_payload(
        "00000000-0000-0000-0000-000000000001".parse().unwrap(),
        7,
        " recommendation-1 ",
        Some(14),
        None,
        22,
        &"a".repeat(64),
        &"b".repeat(64),
    );
    assert_eq!(
        payload,
        format!(
            "v3|00000000-0000-0000-0000-000000000001|7|recommendation-1|14|none|22|{}|{}",
            "a".repeat(64),
            "b".repeat(64)
        )
    );
}

#[test]
fn ospf_cost_is_monotonic_across_the_full_operator_bandwidth_range() {
    let policy = OspfCostPolicy::default();
    let mut previous = u16::MAX;
    for bandwidth_mbps in MIN_TUNNEL_BANDWIDTH_MBPS..=MAX_TUNNEL_BANDWIDTH_MBPS {
        let cost = ospf_cost(
            policy,
            TunnelObservation {
                latency_ms: 20.0,
                packet_loss_ratio: 0.0,
                bandwidth_mbps,
                preference: 1.0,
            },
        );
        assert!(cost <= previous, "cost increased at {bandwidth_mbps} Mbps");
        previous = cost;
    }
}

#[test]
fn arbitrary_bandwidth_values_use_the_smooth_cost_curve() {
    let cost = |bandwidth_mbps| {
        ospf_cost(
            OspfCostPolicy::default(),
            TunnelObservation {
                latency_ms: 20.0,
                packet_loss_ratio: 0.0,
                bandwidth_mbps,
                preference: 1.0,
            },
        )
    };
    assert_eq!(cost(10), 52);
    assert_eq!(cost(123), 29);
    assert_eq!(cost(1234), 23);
    assert_eq!(cost(9876), 21);
    assert!(cost(99).abs_diff(cost(101)) <= 1);
}

#[test]
fn latency_loss_and_preference_remain_primary_cost_inputs() {
    let policy = OspfCostPolicy::default();
    let healthy = ospf_cost(
        policy,
        TunnelObservation {
            latency_ms: 20.0,
            packet_loss_ratio: 0.0,
            bandwidth_mbps: 100,
            preference: 1.0,
        },
    );
    let unhealthy = ospf_cost(
        policy,
        TunnelObservation {
            latency_ms: 120.0,
            packet_loss_ratio: 0.1,
            bandwidth_mbps: 10_000,
            preference: 1.0,
        },
    );
    let preferred = ospf_cost(
        policy,
        TunnelObservation {
            latency_ms: 20.0,
            packet_loss_ratio: 0.0,
            bandwidth_mbps: 100,
            preference: 2.0,
        },
    );
    assert!(unhealthy > healthy);
    assert!(preferred < healthy);
}

#[test]
fn planned_ospf_cost_is_optional_and_computed_next_to_operator_inputs() {
    let without_ospf = plan_tunnel(&plan_input(
        TunnelKind::Gre,
        RuntimeTunnelManager::AgentBuiltin,
    ))
    .unwrap();
    assert_eq!(without_ospf.recommended_ospf_cost, None);

    let mut input = plan_input(TunnelKind::Gre, RuntimeTunnelManager::AgentBuiltin);
    input.ospf = Some(ospf_config());
    let expected = ospf_cost(
        OspfCostPolicy::default(),
        TunnelObservation {
            latency_ms: 20.0,
            packet_loss_ratio: 0.01,
            bandwidth_mbps: 1234,
            preference: 1.0,
        },
    );
    assert_eq!(
        plan_tunnel(&input).unwrap().recommended_ospf_cost,
        Some(expected)
    );
}

#[test]
fn topology_identity_ignores_policy_only_plan_edits() {
    let plan_id = "00000000-0000-4000-8000-000000000001".parse().unwrap();
    let mut plan = plan_tunnel(&plan_input(
        TunnelKind::Gre,
        RuntimeTunnelManager::AgentBuiltin,
    ))
    .unwrap();
    let identity = tunnel_topology_identity_hash(plan_id, &plan);

    plan.bandwidth_mbps = 9_999;
    plan.left_mtu = Some(1_400);
    plan.right_mtu = Some(1_400);
    plan.ospf = Some(ospf_config());
    plan.recommended_ospf_cost = Some(42);

    assert_eq!(tunnel_topology_identity_hash(plan_id, &plan), identity);
}

#[test]
fn runtime_evidence_identity_tracks_runtime_policy_and_credential_generation() {
    let plan_id = "00000000-0000-4000-8000-000000000001".parse().unwrap();
    let mut plan = plan_tunnel(&plan_input(
        TunnelKind::Wireguard,
        RuntimeTunnelManager::AgentBuiltin,
    ))
    .unwrap();
    let identity = tunnel_runtime_evidence_identity_hash(plan_id, &plan, Some(1));

    plan.bandwidth_mbps = 9_999;
    assert_ne!(
        tunnel_runtime_evidence_identity_hash(plan_id, &plan, Some(1)),
        identity,
        "runtime policy edits must invalidate old adapter/traffic evidence"
    );

    let generation_one = tunnel_runtime_evidence_identity_hash(plan_id, &plan, Some(1));
    assert_ne!(
        tunnel_runtime_evidence_identity_hash(plan_id, &plan, Some(2)),
        generation_one,
        "credential rotation must invalidate old runtime evidence"
    );
}

#[test]
fn topology_identity_changes_with_endpoint_underlay_or_primary_family() {
    let plan_id = "00000000-0000-4000-8000-000000000001".parse().unwrap();
    let plan = plan_tunnel(&plan_input(
        TunnelKind::Gre,
        RuntimeTunnelManager::AgentBuiltin,
    ))
    .unwrap();
    let identity = tunnel_topology_identity_hash(plan_id, &plan);

    let mut changed_endpoint = plan.clone();
    changed_endpoint.right_tunnel_address = "10.255.0.3".to_string();
    assert_ne!(
        tunnel_topology_identity_hash(plan_id, &changed_endpoint),
        identity
    );

    let mut changed_underlay = plan.clone();
    changed_underlay.left_remote_underlay = "198.51.100.11".to_string();
    assert_ne!(
        tunnel_topology_identity_hash(plan_id, &changed_underlay),
        identity,
        "left remote underlay edit must detach prior path evidence"
    );

    let mut changed_underlay = plan.clone();
    changed_underlay.left_local_underlay = Some("198.51.100.12".to_string());
    assert_ne!(
        tunnel_topology_identity_hash(plan_id, &changed_underlay),
        identity,
        "left local underlay edit must detach prior path evidence"
    );

    let mut changed_underlay = plan.clone();
    changed_underlay.right_remote_underlay = "203.0.113.21".to_string();
    assert_ne!(
        tunnel_topology_identity_hash(plan_id, &changed_underlay),
        identity,
        "right remote underlay edit must detach prior path evidence"
    );

    let mut changed_underlay = plan.clone();
    changed_underlay.right_local_underlay = Some("203.0.113.22".to_string());
    assert_ne!(
        tunnel_topology_identity_hash(plan_id, &changed_underlay),
        identity,
        "right local underlay edit must detach prior path evidence"
    );

    let mut changed_family = plan;
    changed_family.latency_primary_family = TunnelAddressFamily::Ipv6;
    assert_ne!(
        tunnel_topology_identity_hash(plan_id, &changed_family),
        identity
    );
}

#[test]
fn tunnel_plan_rejects_ambiguous_identity_and_underlay() {
    let mut same_endpoint = plan_input(TunnelKind::Gre, RuntimeTunnelManager::AgentBuiltin);
    same_endpoint.right_client_id = same_endpoint.left_client_id.clone();
    assert_eq!(
        plan_tunnel(&same_endpoint),
        Err(NetworkPlanError::InvalidTunnelEndpoints)
    );

    let mut malformed_underlay = plan_input(TunnelKind::Gre, RuntimeTunnelManager::AgentBuiltin);
    malformed_underlay.right_remote_underlay = "not-an-address".to_string();
    assert_eq!(
        plan_tunnel(&malformed_underlay),
        Err(NetworkPlanError::InvalidUnderlayAddress)
    );

    let mut mixed_underlay = plan_input(
        TunnelKind::Wireguard,
        RuntimeTunnelManager::ExternalObserved,
    );
    mixed_underlay.right_remote_underlay = "2001:db8::20".to_string();
    mixed_underlay.right_local_underlay = Some("10.0.1.20".to_string());
    assert_eq!(
        plan_tunnel(&mixed_underlay),
        Err(NetworkPlanError::InvalidUnderlayAddress)
    );

    let mut native_ipv6_underlay = plan_input(TunnelKind::Gre, RuntimeTunnelManager::AgentBuiltin);
    native_ipv6_underlay.left_remote_underlay = "2001:db8::10".to_string();
    native_ipv6_underlay.right_remote_underlay = "2001:db8::20".to_string();
    assert_eq!(
        plan_tunnel(&native_ipv6_underlay),
        Err(NetworkPlanError::InvalidUnderlayAddress)
    );

    let mut observed_ipv6_underlay = plan_input(
        TunnelKind::Wireguard,
        RuntimeTunnelManager::ExternalObserved,
    );
    observed_ipv6_underlay.left_remote_underlay = "2001:db8::10".to_string();
    observed_ipv6_underlay.right_remote_underlay = "2001:db8::20".to_string();
    assert!(plan_tunnel(&observed_ipv6_underlay).is_ok());
}

#[test]
fn tunnel_plan_keeps_nat_remote_destinations_independent_from_local_sources() {
    let mut input = plan_input(TunnelKind::Gre, RuntimeTunnelManager::AgentBuiltin);
    input.left_remote_underlay = "203.0.113.20".to_string();
    input.left_local_underlay = Some("10.0.0.10".to_string());
    input.right_remote_underlay = "198.51.100.10".to_string();
    input.right_local_underlay = Some("10.0.1.20".to_string());

    let plan = plan_tunnel(&input).unwrap();
    let left = render_tunnel_endpoint_config(&plan, TunnelEndpointSide::Left).unwrap();
    let right = render_tunnel_endpoint_config(&plan, TunnelEndpointSide::Right).unwrap();

    assert_eq!(left.remote_underlay, "203.0.113.20");
    assert_eq!(left.local_underlay.as_deref(), Some("10.0.0.10"));
    assert_eq!(right.remote_underlay, "198.51.100.10");
    assert_eq!(right.local_underlay.as_deref(), Some("10.0.1.20"));
}

#[test]
fn tunnel_plan_rejects_duplicate_or_disconnected_endpoint_addresses() {
    let mut duplicate = plan_input(TunnelKind::Gre, RuntimeTunnelManager::AgentBuiltin);
    duplicate.ipv4_tunnel = Some(ipv4_pair("10.255.0.1", "10.255.0.1"));
    assert_eq!(plan_tunnel(&duplicate), Err(NetworkPlanError::InvalidCidr));

    let mut disconnected = plan_input(TunnelKind::Gre, RuntimeTunnelManager::AgentBuiltin);
    disconnected.ipv4_tunnel = Some(ipv4_pair("10.255.0.0", "10.255.0.2"));
    assert_eq!(
        plan_tunnel(&disconnected),
        Err(NetworkPlanError::InvalidCidr)
    );
}

#[test]
fn agent_builtin_accepts_its_supported_tunnel_kinds() {
    for kind in [
        TunnelKind::Gre,
        TunnelKind::Ipip,
        TunnelKind::Sit,
        TunnelKind::Fou,
        TunnelKind::Openvpn,
        TunnelKind::Wireguard,
    ] {
        assert!(plan_tunnel(&plan_input(kind, RuntimeTunnelManager::AgentBuiltin)).is_ok());
    }
    for kind in [TunnelKind::TunTap, TunnelKind::Custom] {
        assert_eq!(
            plan_tunnel(&plan_input(kind, RuntimeTunnelManager::AgentBuiltin)),
            Err(NetworkPlanError::UnsupportedRuntimeManagerTunnelKind)
        );
    }
}

#[test]
fn custom_adapter_plans_require_both_endpoint_definition_ids() {
    let valid = plan_input(TunnelKind::Wireguard, RuntimeTunnelManager::CustomAdapter);
    assert!(plan_tunnel(&valid).is_ok());

    let mut missing = valid.clone();
    missing.runtime_control.right_adapter_definition_id = None;
    assert_eq!(
        plan_tunnel(&missing),
        Err(NetworkPlanError::RuntimeTunnelAdapterCommandRequired)
    );
}

#[test]
fn custom_adapter_has_one_canonical_wire_name() {
    assert_eq!(
        serde_json::to_string(&RuntimeTunnelManager::CustomAdapter).unwrap(),
        "\"custom_adapter\""
    );
    assert_eq!(
        serde_json::from_str::<RuntimeTunnelManager>("\"custom_adapter\"").unwrap(),
        RuntimeTunnelManager::CustomAdapter
    );
    assert!(serde_json::from_str::<RuntimeTunnelManager>("\"external_managed_adapter\"").is_err());
}

#[test]
fn external_observed_plans_are_explicit_and_cannot_mutate() {
    let observed = plan_input(
        TunnelKind::Wireguard,
        RuntimeTunnelManager::ExternalObserved,
    );
    assert!(plan_tunnel(&observed).is_ok());

    let mut mutating = observed;
    mutating.runtime_control.traffic_limit.ingress_kbps = Some(1000);
    assert_eq!(
        plan_tunnel(&mutating),
        Err(NetworkPlanError::RuntimeTunnelObservedCannotMutate)
    );

    let mut topology_mutation = plan_input(
        TunnelKind::Wireguard,
        RuntimeTunnelManager::ExternalObserved,
    );
    topology_mutation.runtime_topology.stale_interfaces = vec!["wg-old".to_string()];
    assert_eq!(
        plan_tunnel(&topology_mutation),
        Err(NetworkPlanError::RuntimeTunnelTopologyRequiresAgentBuiltin)
    );
}

#[test]
fn custom_adapter_plans_delegate_topology_mutation_to_the_adapter() {
    let mut input = plan_input(TunnelKind::Wireguard, RuntimeTunnelManager::CustomAdapter);
    input.runtime_topology.routes.push(RuntimeTunnelRoute {
        destination_cidr: "10.60.0.0/16".to_string(),
        ..RuntimeTunnelRoute::default()
    });
    assert_eq!(
        plan_tunnel(&input),
        Err(NetworkPlanError::RuntimeTunnelTopologyRequiresAgentBuiltin)
    );
}

#[test]
fn topology_intent_accepts_only_declared_interfaces_and_routes() {
    let valid = RuntimeTunnelTopologyIntent {
        version: Some("v1".to_string()),
        desired_interfaces: vec!["tunab".to_string(), "tunab-peer".to_string()],
        stale_interfaces: vec!["tunab-old".to_string()],
        routes: vec![RuntimeTunnelRoute {
            destination_cidr: "10.60.0.0/16".to_string(),
            interface_name: Some("tunab".to_string()),
            metric: Some(20),
            ..RuntimeTunnelRoute::default()
        }],
        stale_routes: Vec::new(),
    };
    assert!(validate_runtime_topology_intent(&valid, "tunab").is_ok());

    let mut invalid = valid;
    invalid.desired_interfaces.push("../host".to_string());
    assert_eq!(
        validate_runtime_topology_intent(&invalid, "tunab"),
        Err(NetworkPlanError::InvalidRuntimeTunnelTopology)
    );
}

#[test]
fn endpoint_rendering_is_side_specific_without_generating_daemon_files() {
    let mut input = plan_input(TunnelKind::Gre, RuntimeTunnelManager::AgentBuiltin);
    input.left_mtu = Some(1400);
    input.right_mtu = Some(1450);
    let plan = plan_tunnel(&input).unwrap();
    let left = render_tunnel_endpoint_config(&plan, TunnelEndpointSide::Left).unwrap();
    let right = render_tunnel_endpoint_config(&plan, TunnelEndpointSide::Right).unwrap();
    assert_eq!(left.local_client_id, "edge-a");
    assert_eq!(left.local_tunnel_address, "10.255.0.0");
    assert_eq!(left.local_mtu, Some(1400));
    assert_eq!(right.local_client_id, "edge-b");
    assert_eq!(right.local_tunnel_address, "10.255.0.1");
    assert_eq!(right.local_mtu, Some(1450));
}

#[test]
fn endpoint_allocator_returns_non_overlapping_dual_stack_pairs() {
    let allocation = allocate_tunnel_endpoints(
        Some("10.255.0.0/29"),
        Some("fd00::/125"),
        &["10.255.0.0".to_string(), "fd00::".to_string()],
        true,
        true,
    )
    .unwrap();
    let ipv4 = allocation.ipv4_tunnel.unwrap();
    let ipv6 = allocation.ipv6_tunnel.unwrap();
    assert_ne!(ipv4.left, "10.255.0.0");
    assert_ne!(ipv4.left, ipv4.right);
    assert_ne!(ipv6.left, "fd00::");
    assert_ne!(ipv6.left, ipv6.right);
}

#[test]
fn plan_rejects_out_of_range_bandwidth_and_invalid_ospf_binding() {
    let mut bandwidth = plan_input(TunnelKind::Gre, RuntimeTunnelManager::AgentBuiltin);
    bandwidth.bandwidth_mbps = 10_001;
    assert_eq!(
        plan_tunnel(&bandwidth),
        Err(NetworkPlanError::InvalidBandwidthMbps)
    );

    let mut ospf = plan_input(TunnelKind::Gre, RuntimeTunnelManager::AgentBuiltin);
    ospf.ospf = Some(ospf_config());
    ospf.ospf.as_mut().unwrap().left_adapter_definition_id = Some(String::new());
    assert_eq!(plan_tunnel(&ospf), Err(NetworkPlanError::InvalidOspfConfig));
}

#[test]
fn plan_rejects_invalid_endpoint_mtu_and_enforces_the_ipv6_minimum() {
    let mut too_small = plan_input(TunnelKind::Gre, RuntimeTunnelManager::AgentBuiltin);
    too_small.left_mtu = Some(MIN_TUNNEL_MTU - 1);
    assert_eq!(
        plan_tunnel(&too_small),
        Err(NetworkPlanError::InvalidTunnelMtu)
    );

    let mut ipv6 = plan_input(TunnelKind::Gre, RuntimeTunnelManager::AgentBuiltin);
    ipv6.ipv6_tunnel = Some(TunnelAddressPair {
        left: "fd00::".to_string(),
        right: "fd00::1".to_string(),
        prefix_len: 127,
    });
    ipv6.right_mtu = Some(MIN_IPV6_TUNNEL_MTU - 1);
    assert_eq!(plan_tunnel(&ipv6), Err(NetworkPlanError::InvalidTunnelMtu));
    ipv6.right_mtu = Some(MIN_IPV6_TUNNEL_MTU);
    assert!(plan_tunnel(&ipv6).is_ok());

    let mut sit = plan_input(TunnelKind::Sit, RuntimeTunnelManager::AgentBuiltin);
    sit.left_mtu = Some(MIN_IPV6_TUNNEL_MTU - 1);
    assert_eq!(plan_tunnel(&sit), Err(NetworkPlanError::InvalidTunnelMtu));

    let mut missing = plan_input(TunnelKind::Gre, RuntimeTunnelManager::AgentBuiltin);
    missing.right_mtu = None;
    assert_eq!(
        plan_tunnel(&missing),
        Err(NetworkPlanError::TunnelMtuRequired)
    );

    let mut custom_adapter = plan_input(TunnelKind::Wireguard, RuntimeTunnelManager::CustomAdapter);
    custom_adapter.left_mtu = Some(1420);
    assert_eq!(
        plan_tunnel(&custom_adapter),
        Err(NetworkPlanError::TunnelMtuExternallyOwned)
    );
}

#[test]
fn published_network_wire_names_remain_stable() {
    let ospf = ospf_config();
    let encoded_ospf = serde_json::to_value(&ospf).unwrap();
    assert_eq!(
        encoded_ospf["left_adapter_template_id"],
        LEFT_ROUTING_ADAPTER
    );
    assert_eq!(
        encoded_ospf["right_adapter_template_id"],
        RIGHT_ROUTING_ADAPTER
    );
    assert!(encoded_ospf.get("left_adapter_definition_id").is_none());
    assert!(encoded_ospf.get("right_adapter_definition_id").is_none());

    let command = RuntimeTunnelCommand {
        argv: vec!["/opt/vpsman/routing-cost".to_string()],
        ..RuntimeTunnelCommand::default()
    };
    let adapter = RoutingCostAdapterCommands {
        source: RoutingCostCommandSource::ConfigurationPreset,
        definition_id: LEFT_ROUTING_ADAPTER.to_string(),
        definition_name: "FRR updater".to_string(),
        definition_hash: "a".repeat(64),
        status: command.clone(),
        update: command,
    };
    let encoded_adapter = serde_json::to_value(&adapter).unwrap();
    assert_eq!(encoded_adapter["template_id"], LEFT_ROUTING_ADAPTER);
    assert_eq!(encoded_adapter["template_name"], "FRR updater");
    assert_eq!(encoded_adapter["source"], "configuration_preset");
    assert!(encoded_adapter.get("definition_id").is_none());
    assert!(encoded_adapter.get("definition_name").is_none());

    let result = RoutingCostAdapterJobResult {
        contract_version: ROUTING_COST_ADAPTER_CONTRACT_VERSION,
        operation: RoutingCostAdapterOperation::Status,
        plan_id: "00000000-0000-4000-8000-000000000099".to_string(),
        endpoint_side: TunnelEndpointSide::Left,
        client_id: "edge-a".to_string(),
        adapter_definition_id: LEFT_ROUTING_ADAPTER.to_string(),
        adapter_definition_hash: "a".repeat(64),
        previous_cost: None,
        current_cost: 20,
        message: None,
    };
    let encoded_result = serde_json::to_value(result).unwrap();
    assert_eq!(encoded_result["adapter_template_id"], LEFT_ROUTING_ADAPTER);
    assert!(encoded_result.get("adapter_definition_id").is_none());
}

#[test]
fn internal_definition_aliases_are_accepted_without_changing_canonical_output() {
    let decoded: TunnelOspfConfig = serde_json::from_value(serde_json::json!({
        "mode": "reviewed",
        "planned_latency_ms": 20.0,
        "planned_packet_loss_ratio": 0.01,
        "preference": 1.0,
        "policy": OspfCostPolicy::default(),
        "min_cost_delta": 5,
        "healthy_windows": 2,
        "left_adapter_definition_id": LEFT_ROUTING_ADAPTER,
        "right_adapter_definition_id": RIGHT_ROUTING_ADAPTER
    }))
    .unwrap();
    assert_eq!(
        decoded.left_adapter_definition_id.as_deref(),
        Some(LEFT_ROUTING_ADAPTER)
    );
    assert_eq!(
        decoded.right_adapter_definition_id.as_deref(),
        Some(RIGHT_ROUTING_ADAPTER)
    );
    let encoded = serde_json::to_value(decoded).unwrap();
    assert!(encoded.get("left_adapter_definition_id").is_none());
    assert_eq!(encoded["left_adapter_template_id"], LEFT_ROUTING_ADAPTER);
}
