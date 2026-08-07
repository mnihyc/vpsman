use super::*;

#[test]
fn parses_all_vnstat_resolutions_from_one_v2_snapshot() {
    let payload = serde_json::json!({
        "vnstatversion": "2.13",
        "jsonversion": "2",
        "interfaces": [{
            "name": "eth0",
            "created": {"timestamp": 1_722_000_000_u64},
            "updated": {"timestamp": 1_722_474_135_u64},
            "traffic": {
                "fiveminute": [
                    {"timestamp": 1_722_473_700_u64, "rx": 120, "tx": 45},
                    {"timestamp": 1_722_474_000_u64, "rx": 30, "tx": 15}
                ],
                "hour": [
                    {"timestamp": 1_722_470_400_u64, "rx": 3_600, "tx": 1_800}
                ],
                "day": [
                    {"timestamp": 1_722_384_000_u64, "rx": 86_400, "tx": 43_200},
                    {"timestamp": 1_722_470_400_u64, "rx": 7_200, "tx": 3_600}
                ]
            }
        }]
    });

    let (source, buckets) =
        parse_vnstat_payload(&payload, "eth0", 1_722_384_000).unwrap();

    assert_eq!(source.database_created_unix, Some(1_722_000_000));
    assert_eq!(source.source_updated_unix, Some(1_722_474_135));
    assert!(buckets.iter().any(|bucket| bucket.duration_secs == 300));
    assert!(buckets.iter().any(|bucket| bucket.duration_secs == 3_600));
    assert!(buckets.iter().any(|bucket| bucket.duration_secs == 86_400));
    assert!(buckets.iter().any(|bucket| {
        bucket.start_unix == 1_722_474_000 && bucket.duration_secs == 120
    }));
}

#[test]
fn day_parser_uses_next_timestamp_for_dst_length() {
    let payload = serde_json::json!({
        "jsonversion": 2,
        "interfaces": [{
            "name": "eth0",
            "created": {"timestamp": 1_700_006_400_u64},
            "updated": {"timestamp": 1_700_207_640_u64},
            "traffic": {
                "day": [
                    {"timestamp": 1_700_006_400_u64, "rx": 1, "tx": 2},
                    {"timestamp": 1_700_089_200_u64, "rx": 3, "tx": 4}
                ]
            }
        }]
    });

    let (_, buckets) = parse_vnstat_payload(&payload, "eth0", 1_700_006_400).unwrap();
    assert_eq!(buckets[0].duration_secs, 82_800);
}

#[test]
fn request_requires_unique_minute_aligned_interfaces_and_range() {
    // The validator receives the real wall clock and floors it internally.
    let now = 1_722_474_037;
    let current_minute = floor_minute(now);
    assert!(validate_request_at(&["eth0".to_string()], 1_722_470_400, now).is_ok());
    assert!(validate_request_at(&["eth0".to_string()], current_minute, now).is_err());
    assert!(validate_request_at(
        &["eth0".to_string(), "eth0".to_string()],
        1_722_470_400,
        now,
    )
    .is_err());
    assert!(validate_request_at(&["eth0".to_string()], 1_722_470_401, now).is_err());
    assert!(validate_request_at(&["eth 0".to_string()], 1_722_470_400, now).is_err());
}

#[test]
fn parser_rejects_non_v2_json() {
    let payload = serde_json::json!({"jsonversion": "1", "interfaces": []});
    assert!(parse_vnstat_payload(&payload, "eth0", 1_722_470_400).is_err());
}
