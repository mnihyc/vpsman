use super::*;
use serde_json::json;

fn rule() -> PortForwardRule {
    serde_json::from_value(json!({
        "id": Uuid::new_v4(), "revision":1, "name":"pool", "protocol":"tcp", "mappings":[],
        "pool": {"incoming":[{"start":8443,"end":8443}],"strategy":"round_robin", "upstreams":[
            {"id":Uuid::new_v4(),"target_ip":"192.0.2.1","ports":{"start":1000,"end":1002},"weight":2,"role":"primary","enabled":true},
            {"id":Uuid::new_v4(),"target_ip":"192.0.2.2","ports":{"start":2000,"end":2000},"weight":1,"role":"primary","enabled":true}
        ]}
    })).unwrap()
}

fn config(rules: Vec<PortForwardRule>) -> AgentPortForwardingConfig {
    AgentPortForwardingConfig {
        schema_version: 3,
        desired_hash: port_forwarding_desired_hash(&rules),
        rules,
        ..Default::default()
    }
}

#[test]
fn imbalance_is_explicit_and_old_mapping_semantics_remain() {
    let rule = rule();
    validate_port_forwarding_config(&config(vec![rule.clone()])).unwrap();
    assert_eq!(
        pair_port_expressions("8443", "1000-1002"),
        Err(PortForwardValidationError::TargetCardinalityMismatch)
    );
    for version in [1, 2] {
        let mut old = config(vec![rule.clone()]);
        old.schema_version = version;
        assert_eq!(
            validate_port_forwarding_config(&old),
            Err(PortForwardValidationError::SchemaUnsupported)
        );
    }
    let mut fixed = rule.clone();
    fixed.id = Uuid::new_v4();
    fixed.pool = None;
    fixed.target_ip = Some("192.0.2.9".parse().unwrap());
    fixed.mappings = pair_port_expressions("8443", "443").unwrap();
    assert_eq!(
        validate_port_forwarding_config(&config(vec![rule, fixed])),
        Err(PortForwardValidationError::CrossRuleOverlap)
    );
}

#[test]
fn invalid_pool_ranges_weights_and_modes_are_rejected_before_expansion() {
    let original = rule();
    for change in 0..8 {
        let mut r = original.clone();
        let p = r.pool.as_mut().unwrap();
        match change {
            0 => p.upstreams[0].ports.end = 0,
            1 => p.upstreams[0].weight = 0,
            2 => p.upstreams[0].weight = u32::MAX,
            3 => p.upstreams[0].target_ip = "2001:db8::1".parse().unwrap(),
            4 => p.upstreams[1] = p.upstreams[0].clone(),
            5 => {
                for row in &mut p.upstreams {
                    row.enabled = false;
                }
            }
            6 => p.upstreams[0].failure_policy = Some(PortForwardFailurePolicy::Off),
            _ => {
                r.mode = PortForwardMode::Redirect;
                r.address_family = Some(PortForwardAddressFamily::Both);
                r.masquerade = false;
            }
        }
        assert!(
            validate_port_forwarding_config(&config(vec![r])).is_err(),
            "case {change}"
        );
    }
    let mut large = original;
    large.pool.as_mut().unwrap().upstreams[0].ports = PortRange {
        start: 1,
        end: 65535,
    };
    assert_eq!(
        validate_port_forwarding_config(&config(vec![large])),
        Err(PortForwardValidationError::ProgramTooLarge)
    );
}

#[test]
fn custom_pool_needs_commands_but_no_host_or_adapter_support_declaration() {
    let mut r = rule();
    let command = json!({"argv":["/not-installed/forwarder", "{forwarding_type}", "{rule_config_json}"], "max_timeout_secs":30,"max_output_bytes":16384});
    r.mode = PortForwardMode::CustomAdapter;
    r.masquerade = false;
    r.adapter = Some(serde_json::from_value(json!({
        "template_id":Uuid::new_v4(),"template_name":"custom","definition_hash":"hash","contract_version":2,
        "apply":command,"remove":command,"status":command
    })).unwrap());
    let p = r.pool.as_mut().unwrap();
    p.strategy = PortForwardPoolStrategy::LeastConnections;
    p.connect_timeout_secs = Some(3);
    p.retry_policy = Some(PortForwardRetryPolicy::Off);
    p.upstreams[0].failure_policy = Some(PortForwardFailurePolicy::Temporary {
        threshold: 2,
        window_secs: 10,
        retry_after_secs: 10,
    });
    // Custom execution is not subject to nft's expanded 32-bit selector width.
    p.upstreams[0].weight = u32::MAX;
    p.upstreams[1].target_ip = "::1".parse().unwrap();
    validate_port_forwarding_config(&config(vec![r.clone()])).unwrap();
    r.pool.as_mut().unwrap().connect_timeout_secs = Some(0);
    assert!(validate_port_forward_rule(&r).is_err());
    r.pool.as_mut().unwrap().connect_timeout_secs = Some(3);
    r.adapter = None;
    assert!(validate_port_forward_rule(&r).is_err());
}

#[test]
fn adapter_request_identifies_exact_inputs_and_omits_executable_commands() {
    let mut r = rule();
    let first = PortForwardAdapterRequest::new("v-test", &r);
    assert_eq!(
        first,
        serde_json::from_slice(&serde_json::to_vec(&first).unwrap()).unwrap()
    );
    assert_ne!(
        first.config_hash,
        PortForwardAdapterRequest::new("v-other", &r).config_hash
    );
    r.pool.as_mut().unwrap().upstreams[0].weight += 1;
    assert_ne!(
        first.config_hash,
        PortForwardAdapterRequest::new("v-test", &r).config_hash
    );
    assert!(serde_json::to_value(first).unwrap()["rule"]
        .get("adapter")
        .is_none());
}
