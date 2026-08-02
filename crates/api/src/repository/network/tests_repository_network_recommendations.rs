use super::compare_optional_timestamps_desc;
use std::cmp::Ordering;

#[test]
fn recommendation_ordering_handles_mixed_timestamp_formats_and_missing_evidence() {
    assert_eq!(
        compare_optional_timestamps_desc(Some("1770000000"), Some("2026-01-01T00:00:00Z"),),
        Ordering::Less,
    );
    assert_eq!(
        compare_optional_timestamps_desc(Some("1770000000"), None),
        Ordering::Less,
    );
}
