use super::*;

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
