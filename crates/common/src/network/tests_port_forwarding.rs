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
        target_ip: "192.0.2.8".parse().unwrap(),
        mappings: pair_port_expressions("80", "8080").unwrap(),
        masquerade: true,
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
    ipv6.target_ip = "2001:db8::8".parse().unwrap();
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
    for rule_index in 0..43_u16 {
        let first = rule_index * 256 + 1;
        let mappings = (0..256_u16)
            .map(|offset| PortForwardMapping {
                incoming: PortRange {
                    start: first + offset,
                    end: first + offset,
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
            target_ip: "192.0.2.8".parse().unwrap(),
            mappings,
            masquerade: true,
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
