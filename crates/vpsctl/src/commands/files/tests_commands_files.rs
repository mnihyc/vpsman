use super::parse_file_mode;

#[test]
fn parses_file_modes_as_octal_when_prefixed() {
    assert_eq!(parse_file_mode("0644").unwrap(), 0o644);
    assert_eq!(parse_file_mode("0o600").unwrap(), 0o600);
    assert_eq!(parse_file_mode("644").unwrap(), 0o644);
    assert_eq!(parse_file_mode("420").unwrap(), 0o420);
    assert!(parse_file_mode("1000").is_err());
    assert!(parse_file_mode("888").is_err());
    assert!(parse_file_mode("not-a-mode").is_err());
}
