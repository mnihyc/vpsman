use super::*;
use vpsman_common::{
    plan_tunnel, render_tunnel_endpoint_config, AgentNetworkConfig, AgentNetworkPreset,
    BandwidthTier, OspfCostPolicy, TunnelEndpointSide, TunnelKind, TunnelPlan, TunnelPlanInput,
};

#[test]
fn renders_debian_ifupdown2_bird2_preset_hooks() {
    let plan = test_plan();
    let endpoint = render_tunnel_endpoint_config(&plan, TunnelEndpointSide::Left).unwrap();
    let context = NetworkHookContext {
        plan: &plan,
        endpoint: &endpoint,
    };
    let config = AgentNetworkConfig {
        validate_enabled: true,
        reload_enabled: true,
        preset: Some(AgentNetworkPreset::DebianIfupdown2Bird2),
        ..AgentNetworkConfig::default()
    };

    let validation = validation_hook_specs(&config, context);
    assert_eq!(validation.len(), 2);
    assert_eq!(
        validation[0].label,
        "preset_debian_ifupdown2_bird2_ifupdown_syntax_validate"
    );
    assert_eq!(validation[0].argv, ["/usr/sbin/ifreload", "-a", "-s"]);
    assert_eq!(
        validation[1].argv,
        ["/usr/sbin/bird", "-p", "-c", "/etc/bird/bird.conf"]
    );

    let endpoint = render_tunnel_endpoint_config(&plan, TunnelEndpointSide::Left).unwrap();
    let reload = reload_hook_specs(
        &config,
        NetworkHookContext {
            plan: &plan,
            endpoint: &endpoint,
        },
    );
    assert_eq!(reload.len(), 2);
    assert_eq!(reload[0].argv, ["/usr/sbin/ifreload", "-a"]);
    assert_eq!(reload[1].argv, ["/usr/sbin/birdc", "configure"]);

    let endpoint = render_tunnel_endpoint_config(&plan, TunnelEndpointSide::Left).unwrap();
    let pre_rollback = pre_rollback_hook_specs(
        &config,
        NetworkHookContext {
            plan: &plan,
            endpoint: &endpoint,
        },
    );
    assert_eq!(pre_rollback.len(), 1);
    assert_eq!(
        pre_rollback[0].label,
        "preset_debian_ifupdown2_bird2_ifupdown_pre_rollback"
    );
    assert_eq!(pre_rollback[0].argv, ["/usr/sbin/ifdown", "-f", "tunlr"]);

    let endpoint = render_tunnel_endpoint_config(&plan, TunnelEndpointSide::Left).unwrap();
    let bird2_validation = bird2_validation_hook_specs(
        &config,
        NetworkHookContext {
            plan: &plan,
            endpoint: &endpoint,
        },
    );
    assert_eq!(bird2_validation.len(), 1);
    assert_eq!(
        bird2_validation[0].label,
        "preset_debian_ifupdown2_bird2_bird2_parse_validate"
    );

    let endpoint = render_tunnel_endpoint_config(&plan, TunnelEndpointSide::Left).unwrap();
    let bird2_reload = bird2_reload_hook_specs(
        &config,
        NetworkHookContext {
            plan: &plan,
            endpoint: &endpoint,
        },
    );
    assert_eq!(bird2_reload.len(), 1);
    assert_eq!(bird2_reload[0].argv, ["/usr/sbin/birdc", "configure"]);
}

#[test]
fn renders_debian_ifupdown_bird2_legacy_preset_hooks() {
    let plan = test_plan();
    let endpoint = render_tunnel_endpoint_config(&plan, TunnelEndpointSide::Left).unwrap();
    let config = AgentNetworkConfig {
        validate_enabled: true,
        reload_enabled: true,
        preset: Some(AgentNetworkPreset::DebianIfupdownBird2),
        ..AgentNetworkConfig::default()
    };

    let validation = validation_hook_specs(
        &config,
        NetworkHookContext {
            plan: &plan,
            endpoint: &endpoint,
        },
    );
    assert_eq!(validation.len(), 2);
    assert_eq!(
        validation[0].label,
        "preset_debian_ifupdown_bird2_ifupdown_no_act_validate"
    );
    assert_eq!(validation[0].argv, ["/sbin/ifup", "--no-act", "tunlr"]);
    assert_eq!(
        validation[1].argv,
        ["/usr/sbin/bird", "-p", "-c", "/etc/bird/bird.conf"]
    );

    let reload = reload_hook_specs(
        &config,
        NetworkHookContext {
            plan: &plan,
            endpoint: &endpoint,
        },
    );
    assert_eq!(reload.len(), 3);
    assert_eq!(reload[0].argv, ["/sbin/ifdown", "-f", "tunlr"]);
    assert_eq!(reload[1].argv, ["/sbin/ifup", "tunlr"]);
    assert_eq!(reload[2].argv, ["/usr/sbin/birdc", "configure"]);

    let pre_rollback = pre_rollback_hook_specs(
        &config,
        NetworkHookContext {
            plan: &plan,
            endpoint: &endpoint,
        },
    );
    assert_eq!(pre_rollback.len(), 1);
    assert_eq!(pre_rollback[0].argv, ["/sbin/ifdown", "-f", "tunlr"]);
}

#[test]
fn renders_netplan_and_systemd_networkd_bird2_preset_hooks() {
    let plan = test_plan();
    let endpoint = render_tunnel_endpoint_config(&plan, TunnelEndpointSide::Left).unwrap();

    let netplan_config = AgentNetworkConfig {
        validate_enabled: true,
        reload_enabled: true,
        preset: Some(AgentNetworkPreset::DebianNetplanBird2),
        ..AgentNetworkConfig::default()
    };
    let netplan_validation = validation_hook_specs(
        &netplan_config,
        NetworkHookContext {
            plan: &plan,
            endpoint: &endpoint,
        },
    );
    assert_eq!(
        netplan_validation[0].argv,
        ["/usr/sbin/netplan", "generate"]
    );
    let netplan_reload = reload_hook_specs(
        &netplan_config,
        NetworkHookContext {
            plan: &plan,
            endpoint: &endpoint,
        },
    );
    assert_eq!(netplan_reload[0].argv, ["/usr/sbin/netplan", "apply"]);
    assert_eq!(netplan_reload[1].argv, ["/usr/sbin/birdc", "configure"]);

    let networkd_config = AgentNetworkConfig {
        validate_enabled: true,
        reload_enabled: true,
        preset: Some(AgentNetworkPreset::DebianSystemdNetworkdBird2),
        ..AgentNetworkConfig::default()
    };
    let networkd_validation = validation_hook_specs(
        &networkd_config,
        NetworkHookContext {
            plan: &plan,
            endpoint: &endpoint,
        },
    );
    assert_eq!(networkd_validation[0].argv[0], "/usr/bin/systemd-analyze");
    assert_eq!(networkd_validation[0].argv[1], "verify");
    let networkd_reload = reload_hook_specs(
        &networkd_config,
        NetworkHookContext {
            plan: &plan,
            endpoint: &endpoint,
        },
    );
    assert_eq!(networkd_reload[0].argv, ["/usr/bin/networkctl", "reload"]);
    assert_eq!(
        networkd_reload[1].argv,
        ["/usr/bin/networkctl", "reconfigure", "tunlr"]
    );
}

#[test]
fn renders_explicit_hook_placeholders_before_execution() {
    let plan = test_plan();
    let endpoint = render_tunnel_endpoint_config(&plan, TunnelEndpointSide::Left).unwrap();
    let config = AgentNetworkConfig {
        validate_enabled: true,
        ifupdown_validate_argv: vec![
            "/opt/vpsman/validate-iface".to_string(),
            "{interface}".to_string(),
            "{plan}".to_string(),
            "{local_client_id}".to_string(),
            "{peer_client_id}".to_string(),
        ],
        ..AgentNetworkConfig::default()
    };

    let specs = validation_hook_specs(
        &config,
        NetworkHookContext {
            plan: &plan,
            endpoint: &endpoint,
        },
    );

    assert_eq!(specs.len(), 1);
    assert_eq!(specs[0].label, "ifupdown_validate");
    assert_eq!(
        specs[0].argv,
        [
            "/opt/vpsman/validate-iface",
            "tunlr",
            "left-right",
            "left-a",
            "right-b"
        ]
    );
}

fn test_plan() -> TunnelPlan {
    plan_tunnel(&TunnelPlanInput {
        name: "left-right".to_string(),
        interface_name: "tunlr".to_string(),
        kind: TunnelKind::Gre,
        runtime_control: Default::default(),
        runtime_topology: Default::default(),
        left_client_id: "left-a".to_string(),
        right_client_id: "right-b".to_string(),
        left_underlay: "198.51.100.10".to_string(),
        right_underlay: "203.0.113.20".to_string(),
        address_pool_cidr: "10.255.0.0/30".to_string(),
        reserved_addresses: Vec::new(),
        ipv4_tunnel: Some(vpsman_common::TunnelAddressPair {
            left: "10.255.0.0".to_string(),
            right: "10.255.0.1".to_string(),
            prefix_len: 31,
        }),
        ipv6_address_pool_cidr: None,
        ipv6_tunnel: None,
        latency_primary_family: Default::default(),
        bandwidth: BandwidthTier::M100,
        latency_ms: 15.0,
        packet_loss_ratio: 0.0,
        preference: 1.0,
        ospf_policy: OspfCostPolicy::default(),
    })
    .unwrap()
}
