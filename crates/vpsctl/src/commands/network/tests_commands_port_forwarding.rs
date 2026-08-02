use super::*;

#[test]
fn parses_shifted_ranges_before_submission() {
    assert!(pair_port_expressions("80,1000-1002", "8080,2000-2002").is_ok());
    assert!(pair_port_expressions("1000-1002", "2000-2001").is_err());
}
