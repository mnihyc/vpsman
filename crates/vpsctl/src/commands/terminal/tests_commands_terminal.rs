use super::{terminal_input_data, validate_terminal_client_id};

#[test]
fn terminal_input_text_is_encoded_and_validated() {
    assert_eq!(
        terminal_input_data(Some("id\n".to_string()), None).unwrap(),
        "aWQK"
    );
    assert!(terminal_input_data(Some("id".to_string()), Some("aWQ=".to_string())).is_err());
    assert!(terminal_input_data(None, None).is_err());
}

#[test]
fn terminal_input_client_id_is_path_safe() {
    assert!(validate_terminal_client_id("edge-a").is_ok());
    assert!(validate_terminal_client_id("edge/a").is_err());
    assert!(validate_terminal_client_id("edge a").is_err());
}
