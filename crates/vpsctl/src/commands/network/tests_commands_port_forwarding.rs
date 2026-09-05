use super::*;

#[test]
fn mode_payloads_preserve_native_defaults_and_adapter_ownership() {
    let mut payload = serde_json::json!({});
    insert_mode_fields(
        &mut payload,
        &PortForwardModeArgs::default(),
        Some("192.0.2.8".parse().unwrap()),
        None,
        false,
    )
    .unwrap();
    assert_eq!(payload["mode"], "dnat");
    assert_eq!(payload["masquerade"], true);
    assert!(payload["address_family"].is_null());
    assert!(insert_mode_fields(
        &mut payload,
        &PortForwardModeArgs::default(),
        None,
        None,
        false
    )
    .is_err());

    let redirect = PortForwardModeArgs {
        mode: PortForwardModeArg::Redirect,
        ..Default::default()
    };
    insert_mode_fields(&mut payload, &redirect, None, None, false).unwrap();
    assert_eq!(payload["address_family"], "ipv4");
    assert_eq!(payload["masquerade"], false);
    assert!(insert_mode_fields(
        &mut payload,
        &redirect,
        Some("127.0.0.1".parse().unwrap()),
        None,
        false
    )
    .is_err());

    let custom = PortForwardModeArgs {
        mode: PortForwardModeArg::CustomAdapter,
        adapter_definition_id: Some(Uuid::new_v4()),
        ..Default::default()
    };
    payload["target_hostname"] = serde_json::json!("previous.example");
    insert_mode_fields(&mut payload, &custom, None, None, false).unwrap();
    assert_eq!(payload["mode"], "custom_adapter");
    assert!(payload["target_hostname"].is_null());
    assert!(payload["address_family"].is_null());
    assert_eq!(payload["masquerade"], false);
    assert!(insert_mode_fields(&mut payload, &custom, None, Some("localhost"), false).is_err());
}

#[test]
fn parses_shifted_ranges_before_submission() {
    assert!(pair_port_expressions("80,1000-1002", "8080,2000-2002").is_ok());
    assert!(pair_port_expressions("1000-1002", "2000-2001").is_err());
}

#[test]
fn target_hostname_payload_distinguishes_omission_replacement_and_clear() {
    let mut omitted = serde_json::json!({ "target_ip": "192.0.2.8" });
    insert_target_hostname(&mut omitted, None, false);
    assert!(!omitted.as_object().unwrap().contains_key("target_hostname"));

    let mut replaced = serde_json::json!({ "target_ip": "192.0.2.8" });
    insert_target_hostname(&mut replaced, Some("app.internal"), false);
    assert_eq!(replaced["target_hostname"], "app.internal");

    let mut cleared = serde_json::json!({ "target_ip": "192.0.2.8" });
    insert_target_hostname(&mut cleared, None, true);
    assert!(cleared["target_hostname"].is_null());
}
