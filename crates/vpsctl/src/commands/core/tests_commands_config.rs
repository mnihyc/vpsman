use super::validate_update_input;

#[test]
fn validates_external_update_input() {
    let sha256_hex = "ab".repeat(32);

    validate_update_input("https://updates.example/vpsman-agent", &sha256_hex).unwrap();
    assert!(validate_update_input("http://updates.example/vpsman-agent", &sha256_hex).is_err());
    assert!(validate_update_input("https://updates.example/vpsman-agent", "not-a-hash").is_err());
}
