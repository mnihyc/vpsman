use super::{
    compare_timestamps_desc, parse_timestamp_unix, parse_timestamp_utc, search_pattern,
    timestamp_in_optional_bounds,
};
use std::cmp::Ordering;

#[test]
fn search_pattern_escapes_like_wildcards() {
    assert_eq!(
        search_pattern(&Some(r"edge_%\host".to_string())),
        Some(r"%edge\_\%\\host%".to_string())
    );
    assert_eq!(search_pattern(&Some("   ".to_string())), None);
}

#[test]
fn timestamp_helpers_compare_mixed_wire_formats_chronologically() {
    assert_eq!(parse_timestamp_unix("1970-01-01 00:02:00+00"), Some(120));
    assert_eq!(
        compare_timestamps_desc("120", "1970-01-01T00:01:00Z"),
        Ordering::Less
    );
    assert!(
        parse_timestamp_utc("1970-01-01T00:00:00.1Z") > parse_timestamp_utc("1970-01-01T00:00:00Z")
    );
    assert_eq!(
        parse_timestamp_utc("1970-01-01T01:00:00+01:00"),
        parse_timestamp_utc("1970-01-01T00:00:00Z")
    );
}

#[test]
fn bounded_timestamp_checks_reject_malformed_values() {
    assert!(timestamp_in_optional_bounds("malformed", None, None));
    assert!(!timestamp_in_optional_bounds("malformed", Some(1), None));
    assert!(!timestamp_in_optional_bounds("malformed", None, Some(1)));
}
