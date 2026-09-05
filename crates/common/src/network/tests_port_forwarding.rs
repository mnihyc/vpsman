use super::*;

#[test]
fn parses_single_many_and_corresponding_ranges() {
    assert_eq!(
        pair_port_expressions("80,443,1000-1002", "8080").unwrap(),
        vec![
            PortForwardMapping {
                incoming: PortRange { start: 80, end: 80 },
                target: PortRange {
                    start: 8080,
                    end: 8080
                }
            },
            PortForwardMapping {
                incoming: PortRange {
                    start: 443,
                    end: 443
                },
                target: PortRange {
                    start: 8080,
                    end: 8080
                }
            },
            PortForwardMapping {
                incoming: PortRange {
                    start: 1000,
                    end: 1002
                },
                target: PortRange {
                    start: 8080,
                    end: 8080
                }
            }
        ]
    );
    assert!(pair_port_expressions("1000-1002,2000-2001", "3000-3002,4000-4001").is_ok());
}

#[test]
fn rejects_ambiguous_or_overlapping_expressions() {
    assert_eq!(
        pair_port_expressions("1000-1002", "2000-2001").unwrap_err(),
        PortForwardValidationError::TargetCardinalityMismatch
    );
    assert_eq!(
        parse_port_expression("80,79-81").unwrap_err(),
        PortForwardValidationError::IncomingOverlap
    );
}

#[test]
fn rejects_cross_rule_protocol_and_family_collisions() {
    let base = PortForwardRule {
        id: Uuid::new_v4(),
        revision: 1,
        name: "web".to_string(),
        protocol: PortForwardProtocol::Both,
        target_ip: Some("192.0.2.8".parse().unwrap()),
        mappings: pair_port_expressions("80", "8080").unwrap(),
        masquerade: true,
        mode: PortForwardMode::Dnat,
        address_family: None,
        adapter: None,
    };
    let mut conflicting = base.clone();
    conflicting.id = Uuid::new_v4();
    conflicting.name = "conflict".to_string();
    conflicting.protocol = PortForwardProtocol::Tcp;
    assert_eq!(
        validate_cross_rule_overlaps(&[base.clone(), conflicting]).unwrap_err(),
        PortForwardValidationError::CrossRuleOverlap
    );
    let mut ipv6 = base.clone();
    ipv6.id = Uuid::new_v4();
    ipv6.target_ip = Some("2001:db8::8".parse().unwrap());
    assert!(validate_cross_rule_overlaps(&[base, ipv6]).is_ok());
}

#[test]
fn serde_defaults_keep_old_runtime_configs_valid() {
    let value: AgentPortForwardingConfig = serde_json::from_str("{}").unwrap();
    assert_eq!(value, AgentPortForwardingConfig::default());
}

#[test]
fn rejects_desired_state_that_would_render_an_oversized_program() {
    let mut rules = Vec::new();
    for rule_index in 0..256_u32 {
        let first = rule_index * 256 + 1;
        let mappings = (0..256_u32)
            .filter_map(|offset| u16::try_from(first + offset).ok())
            .map(|port| PortForwardMapping {
                incoming: PortRange {
                    start: port,
                    end: port,
                },
                target: PortRange {
                    start: 8080,
                    end: 8080,
                },
            })
            .collect::<Vec<_>>();
        rules.push(PortForwardRule {
            id: Uuid::new_v4(),
            revision: 1,
            name: format!("rule-{rule_index}"),
            protocol: PortForwardProtocol::Tcp,
            target_ip: Some("192.0.2.8".parse().unwrap()),
            mappings,
            masquerade: true,
            mode: PortForwardMode::Dnat,
            address_family: None,
            adapter: None,
        });
    }
    let config = AgentPortForwardingConfig {
        desired_hash: port_forwarding_desired_hash(&rules),
        rules,
        ..AgentPortForwardingConfig::default()
    };
    assert_eq!(
        validate_port_forwarding_config(&config).unwrap_err(),
        PortForwardValidationError::ProgramTooLarge
    );
}

#[test]
fn dnat_wire_roundtrip_preserves_existing_desired_identity() {
    let wire = r#"[{"id":"018f89ac-a5ec-7d71-a249-7ccddc0a0001","revision":1,"name":"web","protocol":"tcp","target_ip":"192.0.2.8","mappings":[{"incoming":{"start":80,"end":80},"target":{"start":8080,"end":8080}}],"masquerade":true}]"#;
    let rules: Vec<PortForwardRule> = serde_json::from_str(wire).unwrap();
    assert_eq!(serde_json::to_string(&rules).unwrap(), wire);
    assert_eq!(
        port_forwarding_desired_hash(&rules),
        crate::auth::payload_hash(wire.as_bytes())
    );
    assert!(validate_port_forwarding_config(&AgentPortForwardingConfig {
        desired_hash: port_forwarding_desired_hash(&rules),
        rules,
        ..Default::default()
    })
    .is_ok());
}

fn redirect_rule() -> PortForwardRule {
    PortForwardRule {
        id: Uuid::new_v4(),
        revision: 1,
        name: "local".into(),
        mode: PortForwardMode::Redirect,
        address_family: Some(PortForwardAddressFamily::Both),
        protocol: PortForwardProtocol::Tcp,
        target_ip: None,
        mappings: pair_port_expressions("80", "8080").unwrap(),
        masquerade: false,
        adapter: None,
    }
}

#[test]
fn dual_family_redirect_claims_both_native_families_but_not_custom_listeners() {
    let redirect = redirect_rule();
    assert!(validate_port_forward_rule(&redirect).is_ok());
    let mut dnat = redirect.clone();
    dnat.id = Uuid::new_v4();
    dnat.mode = PortForwardMode::Dnat;
    dnat.address_family = None;
    for address in ["192.0.2.8", "2001:db8::8"] {
        dnat.target_ip = Some(address.parse().unwrap());
        assert_eq!(
            validate_cross_rule_overlaps(&[redirect.clone(), dnat.clone()]),
            Err(PortForwardValidationError::CrossRuleOverlap)
        );
    }
    let mut custom = redirect.clone();
    custom.id = Uuid::new_v4();
    custom.mode = PortForwardMode::CustomAdapter;
    custom.address_family = None;
    custom.target_ip = Some("127.0.0.1".parse().unwrap());
    let command = crate::RuntimeTunnelCommand {
        argv: vec!["/usr/local/bin/forward-adapter".into(), "{rule_id}".into()],
        max_timeout_secs: 30,
        max_output_bytes: 16384,
    };
    custom.adapter = Some(PortForwardAdapterCommands {
        definition_id: Uuid::new_v4(),
        definition_name: "listener".into(),
        definition_hash: "test".into(),
        apply: command.clone(),
        remove: command.clone(),
        status: command,
    });
    assert!(validate_port_forward_rule(&custom).is_ok());
    assert!(validate_cross_rule_overlaps(&[redirect, custom.clone()]).is_ok());
    custom.target_ip = None;
    assert!(validate_port_forward_rule(&custom).is_ok());
}

#[test]
fn new_modes_require_schema_two_and_advertised_capability() {
    let rules = vec![redirect_rule()];
    let mut config = AgentPortForwardingConfig {
        desired_hash: port_forwarding_desired_hash(&rules),
        rules,
        ..Default::default()
    };
    assert_eq!(
        validate_port_forwarding_config(&config),
        Err(PortForwardValidationError::SchemaUnsupported)
    );
    config.schema_version = PORT_FORWARDING_MODES_SCHEMA_VERSION;
    assert!(validate_port_forwarding_config(&config).is_ok());
    let mut capability: PortForwardCapability =
        serde_json::from_str(r#"{"status":"supported"}"#).unwrap();
    assert!(capability.supports_mode(PortForwardMode::Dnat));
    assert!(!capability.supports_mode(PortForwardMode::Redirect));
    capability.schema_version = PORT_FORWARDING_MODES_SCHEMA_VERSION;
    capability.status = PortForwardCapabilityStatus::NftMissing;
    capability.supported_modes = vec![PortForwardMode::CustomAdapter];
    assert!(capability.supports_mode(PortForwardMode::CustomAdapter));
    assert!(!capability.supports_mode(PortForwardMode::Redirect));
}

#[test]
fn cleanup_requests_require_new_schema_and_do_not_conflict_with_replacement_native_owner() {
    let id = Uuid::new_v4();
    let mut config = AgentPortForwardingConfig {
        cleanup_rules: vec![PortForwardCleanupRule {
            rule_id: id,
            revision: 2,
        }],
        ..Default::default()
    };
    assert_eq!(
        validate_port_forwarding_config(&config),
        Err(PortForwardValidationError::SchemaUnsupported)
    );
    config.schema_version = PORT_FORWARDING_MODES_SCHEMA_VERSION;
    assert!(validate_port_forwarding_config(&config).is_ok());
    let mut redirect = redirect_rule();
    redirect.id = id;
    config.rules.push(redirect);
    config.desired_hash = port_forwarding_desired_hash(&config.rules);
    assert!(validate_port_forwarding_config(&config).is_ok());
    config.cleanup_rules.push(config.cleanup_rules[0].clone());
    assert_eq!(
        validate_port_forwarding_config(&config),
        Err(PortForwardValidationError::CleanupRuleIdInvalid)
    );
}
