use super::*;

#[test]
fn parses_date_and_rfc3339_starts_to_utc_minutes() {
    assert_eq!(
        parse_network_traffic_import_start("2024-08-01").unwrap(),
        1_722_470_400
    );
    assert_eq!(
        parse_network_traffic_import_start("2024-08-01T08:00:00+08:00").unwrap(),
        1_722_470_400
    );
}

#[test]
fn rejects_non_minute_rfc3339_start() {
    assert!(parse_network_traffic_import_start("2024-08-01T00:00:01Z").is_err());
}

#[test]
fn accepts_the_first_unix_minute_as_an_import_start() {
    assert_eq!(
        parse_network_traffic_import_start("1970-01-01T00:01:00Z").unwrap(),
        60
    );
}
