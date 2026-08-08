use super::*;

fn utc_calendar_config() -> VnstatCalendarConfig {
    VnstatCalendarConfig {
        use_utc: true,
        ..VnstatCalendarConfig::default()
    }
}

fn utc_unix(year: i32, month: u32, day: u32, hour: u32, minute: u32, second: u32) -> u64 {
    u64::try_from(
        Utc.with_ymd_and_hms(year, month, day, hour, minute, second)
            .single()
            .unwrap()
            .timestamp(),
    )
    .unwrap()
}

#[test]
fn vnstat_query_uses_the_supported_iface_flag() {
    let command = vnstat_query_command("/usr/bin/vnstat", "eth0");
    let args = command
        .as_std()
        .get_args()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect::<Vec<_>>();

    assert_eq!(args, ["--json", "--limit", "0", "--iface", "eth0"]);
    assert!(!args.iter().any(|argument| argument == "--interface"));
}

#[test]
fn vnstat_configuration_query_uses_showconfig_once_without_an_interface() {
    let command = vnstat_showconfig_command("/usr/bin/vnstat");
    let args = command
        .as_std()
        .get_args()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect::<Vec<_>>();

    assert_eq!(args, ["--showconfig"]);
}

#[test]
fn parses_effective_vnstat_calendar_configuration_with_default_markers() {
    let output = r#"
# vnStat 2.13 configuration file
;MonthRotate 7
MonthRotateAffectsYears 1
;TrafficlessEntries 0
UseUTC 1
"#;

    assert_eq!(
        parse_vnstat_showconfig(output).unwrap(),
        VnstatCalendarConfig {
            month_rotate: 7,
            month_rotate_affects_years: true,
            use_utc: true,
            trafficless_entries: false,
        }
    );
}

#[test]
fn legacy_configuration_defaults_only_missing_use_utc_to_local_time() {
    let invalid_bool = r#"
MonthRotate 1
MonthRotateAffectsYears 0
TrafficlessEntries 2
UseUTC 0
"#;
    let legacy = r#"
MonthRotate 1
MonthRotateAffectsYears 0
TrafficlessEntries 1
"#;
    let missing_required = r#"
MonthRotate 1
TrafficlessEntries 1
"#;

    assert!(parse_vnstat_showconfig(invalid_bool).is_err());
    assert_eq!(
        parse_vnstat_showconfig(legacy).unwrap(),
        VnstatCalendarConfig {
            use_utc: false,
            ..VnstatCalendarConfig::default()
        }
    );
    assert!(parse_vnstat_showconfig(missing_required).is_err());
}

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
                ],
                "month": [
                    {"timestamp": 1_719_792_000_u64, "rx": 2_592_000, "tx": 1_296_000}
                ],
                "year": [
                    {"timestamp": 1_704_067_200_u64, "rx": 15_552_000, "tx": 7_776_000}
                ]
            }
        }]
    });

    let (source, buckets) =
        parse_vnstat_payload(&payload, "eth0", 1_722_384_000, &utc_calendar_config()).unwrap();

    assert_eq!(source.database_created_unix, Some(1_722_000_000));
    assert_eq!(source.source_updated_unix, Some(1_722_474_135));
    assert!(buckets.iter().any(|bucket| bucket.duration_secs == 300));
    assert!(buckets.iter().any(|bucket| bucket.duration_secs == 3_600));
    assert!(buckets.iter().any(|bucket| bucket.duration_secs == 86_400));
    assert!(buckets.iter().any(|bucket| bucket.rx_bytes == 2_592_000
        && bucket.start_unix == ceil_minute(1_722_000_000).unwrap()
        && bucket.duration_secs > 86_400));
    assert!(buckets.iter().any(|bucket| bucket.rx_bytes == 15_552_000
        && bucket.start_unix == ceil_minute(1_722_000_000).unwrap()
        && bucket.duration_secs > 5 * 86_400));
    assert!(buckets
        .iter()
        .any(|bucket| { bucket.start_unix == 1_722_474_000 && bucket.duration_secs == 120 }));
}

#[test]
fn month_and_year_rows_cover_history_older_than_daily_retention() {
    let payload = serde_json::json!({
        "jsonversion": 2,
        "interfaces": [{
            "name": "eth0",
            "created": {"timestamp": 1_609_459_200_u64},
            "updated": {"timestamp": 1_678_838_400_u64},
            "traffic": {
                "month": [
                    {"timestamp": 1_672_531_200_u64, "rx": 31_000, "tx": 15_500},
                    {"timestamp": 1_675_209_600_u64, "rx": 28_000, "tx": 14_000},
                    {"timestamp": 1_677_628_800_u64, "rx": 14_000, "tx": 7_000}
                ],
                "year": [
                    {"timestamp": 1_609_459_200_u64, "rx": 365_000, "tx": 182_500},
                    {"timestamp": 1_640_995_200_u64, "rx": 365_000, "tx": 182_500},
                    {"timestamp": 1_672_531_200_u64, "rx": 73_000, "tx": 36_500}
                ]
            }
        }]
    });

    let (_, buckets) =
        parse_vnstat_payload(&payload, "eth0", 1_609_459_200, &utc_calendar_config()).unwrap();

    assert!(buckets.iter().any(|bucket| {
        bucket.start_unix == 1_672_531_200 && bucket.duration_secs == 31 * 86_400
    }));
    assert!(buckets.iter().any(|bucket| {
        bucket.start_unix == 1_675_209_600 && bucket.duration_secs == 28 * 86_400
    }));
    assert!(buckets.iter().any(|bucket| {
        bucket.start_unix == 1_609_459_200 && bucket.duration_secs == 365 * 86_400
    }));
}

#[test]
fn first_partial_calendar_period_starts_at_the_first_complete_database_minute() {
    let label = utc_unix(2024, 1, 1, 0, 0, 0);
    let created = utc_unix(2024, 1, 15, 12, 0, 31);
    let available = utc_unix(2024, 1, 15, 12, 1, 0);
    let period_end = utc_unix(2024, 2, 1, 0, 0, 0);
    // This mirrors vnStat 2.13: calendar configuration is absent from JSON,
    // and a first partial month is still labeled with the month's first day.
    let payload = serde_json::json!({
        "vnstatversion": "2.13",
        "jsonversion": "2",
        "interfaces": [{
            "name": "eth0",
            "alias": "",
            "created": {
                "date": {"year": 2024, "month": 1, "day": 15},
                "timestamp": created
            },
            "updated": {
                "date": {"year": 2024, "month": 2, "day": 1},
                "time": {"hour": 0, "minute": 0},
                "timestamp": period_end
            },
            "traffic": {
                "total": {"rx": 17_000, "tx": 8_500},
                "month": [{
                    "id": 1,
                    "date": {"year": 2024, "month": 1},
                    "timestamp": label,
                    "rx": 17_000,
                    "tx": 8_500
                }]
            }
        }]
    });

    let (_, buckets) =
        parse_vnstat_payload(&payload, "eth0", available, &utc_calendar_config()).unwrap();

    assert_eq!(buckets.len(), 1);
    assert_eq!(buckets[0].start_unix, available);
    assert_eq!(u64::from(buckets[0].duration_secs), period_end - available);
    assert_eq!(buckets[0].rx_bytes, 17_000);
    assert_eq!(buckets[0].tx_bytes, 8_500);
}

#[test]
fn month_rotate_uses_real_period_boundaries_in_utc_and_local_time() {
    let utc_config = VnstatCalendarConfig {
        month_rotate: 7,
        use_utc: true,
        ..VnstatCalendarConfig::default()
    };
    let (utc_start, utc_end) = calendar_period_bounds(
        utc_unix(2024, 2, 1, 0, 0, 0),
        CalendarResolution::Month,
        &utc_config,
    )
    .unwrap();
    assert_eq!(utc_start, utc_unix(2024, 2, 7, 0, 0, 0));
    assert_eq!(utc_end, utc_unix(2024, 3, 7, 0, 0, 0));

    let local_label =
        calendar_midnight_unix(NaiveDate::from_ymd_opt(2024, 7, 1).unwrap(), false).unwrap();
    let local_start =
        calendar_midnight_unix(NaiveDate::from_ymd_opt(2024, 7, 7).unwrap(), false).unwrap();
    let local_end =
        calendar_midnight_unix(NaiveDate::from_ymd_opt(2024, 8, 7).unwrap(), false).unwrap();
    let local_config = VnstatCalendarConfig {
        month_rotate: 7,
        use_utc: false,
        ..VnstatCalendarConfig::default()
    };
    assert_eq!(
        calendar_period_bounds(local_label, CalendarResolution::Month, &local_config).unwrap(),
        (local_start, local_end)
    );
}

#[test]
fn month_rotate_affects_years_only_when_vnstat_configures_it() {
    let label = utc_unix(2024, 1, 1, 0, 0, 0);
    let unrotated = VnstatCalendarConfig {
        month_rotate: 7,
        month_rotate_affects_years: false,
        use_utc: true,
        trafficless_entries: true,
    };
    let rotated = VnstatCalendarConfig {
        month_rotate_affects_years: true,
        ..unrotated
    };

    assert_eq!(
        calendar_period_bounds(label, CalendarResolution::Year, &unrotated).unwrap(),
        (utc_unix(2024, 1, 1, 0, 0, 0), utc_unix(2025, 1, 1, 0, 0, 0))
    );
    assert_eq!(
        calendar_period_bounds(label, CalendarResolution::Year, &rotated).unwrap(),
        (utc_unix(2024, 1, 7, 0, 0, 0), utc_unix(2025, 1, 7, 0, 0, 0))
    );
}

#[test]
fn sparse_month_rows_keep_natural_boundaries_instead_of_stretching_to_the_next_row() {
    let january = utc_unix(2023, 1, 1, 0, 0, 0);
    let march = utc_unix(2023, 3, 1, 0, 0, 0);
    let february = utc_unix(2023, 2, 1, 0, 0, 0);
    let cutoff = utc_unix(2023, 4, 1, 0, 0, 0);
    let payload = serde_json::json!({
        "jsonversion": 2,
        "interfaces": [{
            "name": "eth0",
            "created": {"timestamp": january},
            "updated": {"timestamp": cutoff},
            "traffic": {
                "month": [
                    {"id": 1, "date": {"year": 2023, "month": 1}, "timestamp": january, "rx": 31, "tx": 1},
                    {"id": 2, "date": {"year": 2023, "month": 3}, "timestamp": march, "rx": 31, "tx": 2}
                ]
            }
        }]
    });
    let config = VnstatCalendarConfig {
        use_utc: true,
        trafficless_entries: false,
        ..VnstatCalendarConfig::default()
    };

    let (_, buckets) = parse_vnstat_payload(&payload, "eth0", january, &config).unwrap();

    assert_eq!(buckets.len(), 3);
    assert_eq!(
        (buckets[0].start_unix, buckets[0].duration_secs),
        (january, 31 * 86_400)
    );
    assert_eq!(
        (buckets[1].start_unix, buckets[1].duration_secs),
        (february, 28 * 86_400)
    );
    assert_eq!((buckets[1].rx_bytes, buckets[1].tx_bytes), (0, 0));
    assert_eq!(
        (buckets[2].start_unix, buckets[2].duration_secs),
        (march, 31 * 86_400)
    );
    assert_eq!(
        buckets[0].start_unix + u64::from(buckets[0].duration_secs),
        utc_unix(2023, 2, 1, 0, 0, 0)
    );
}

#[test]
fn trafficless_year_gaps_are_emitted_as_explicit_zero_buckets() {
    let year_2021 = utc_unix(2021, 1, 1, 0, 0, 0);
    let year_2022 = utc_unix(2022, 1, 1, 0, 0, 0);
    let year_2023 = utc_unix(2023, 1, 1, 0, 0, 0);
    let cutoff = utc_unix(2024, 1, 1, 0, 0, 0);
    let payload = serde_json::json!({
        "jsonversion": 2,
        "interfaces": [{
            "name": "eth0",
            "created": {"timestamp": year_2021},
            "updated": {"timestamp": cutoff},
            "traffic": {
                "year": [
                    {"timestamp": year_2021, "rx": 365, "tx": 100},
                    {"timestamp": year_2023, "rx": 730, "tx": 200}
                ]
            }
        }]
    });
    let config = VnstatCalendarConfig {
        use_utc: true,
        trafficless_entries: false,
        ..VnstatCalendarConfig::default()
    };

    let (_, buckets) = parse_vnstat_payload(&payload, "eth0", year_2021, &config).unwrap();

    assert_eq!(buckets.len(), 3);
    let zero = buckets
        .iter()
        .find(|bucket| bucket.start_unix == year_2022)
        .expect("missing trafficless year must be represented");
    assert_eq!(zero.duration_secs, 365 * 86_400);
    assert_eq!((zero.rx_bytes, zero.tx_bytes), (0, 0));
}

#[test]
fn trafficless_does_not_invent_year_buckets_when_years_are_disabled() {
    let january = utc_unix(2024, 1, 1, 0, 0, 0);
    let cutoff = utc_unix(2024, 2, 1, 0, 0, 0);
    let payload = serde_json::json!({
        "jsonversion": 2,
        "interfaces": [{
            "name": "eth0",
            "created": {"timestamp": january},
            "updated": {"timestamp": cutoff},
            "traffic": {
                "month": [
                    {"timestamp": january, "rx": 31, "tx": 10}
                ]
            }
        }]
    });
    let config = VnstatCalendarConfig {
        use_utc: true,
        trafficless_entries: false,
        ..VnstatCalendarConfig::default()
    };

    let (_, buckets) = parse_vnstat_payload(&payload, "eth0", january, &config).unwrap();

    assert_eq!(buckets.len(), 1);
    assert_eq!(buckets[0].duration_secs, 31 * 86_400);
    assert_eq!((buckets[0].rx_bytes, buckets[0].tx_bytes), (31, 10));
}

#[test]
fn trafficless_does_not_turn_expired_leading_history_into_zeroes() {
    let database_created = utc_unix(2020, 1, 1, 0, 0, 0);
    let first_retained_year = utc_unix(2022, 1, 1, 0, 0, 0);
    let cutoff = utc_unix(2024, 1, 1, 0, 0, 0);
    let payload = serde_json::json!({
        "jsonversion": 2,
        "interfaces": [{
            "name": "eth0",
            "created": {"timestamp": database_created},
            "updated": {"timestamp": cutoff},
            "traffic": {
                "year": [
                    {"timestamp": first_retained_year, "rx": 365, "tx": 100},
                    {"timestamp": utc_unix(2023, 1, 1, 0, 0, 0), "rx": 730, "tx": 200}
                ]
            }
        }]
    });
    let config = VnstatCalendarConfig {
        use_utc: true,
        trafficless_entries: false,
        ..VnstatCalendarConfig::default()
    };

    let (_, buckets) = parse_vnstat_payload(&payload, "eth0", database_created, &config).unwrap();

    assert!(buckets
        .iter()
        .all(|bucket| bucket.start_unix >= first_retained_year));
}

#[test]
fn crossing_rotated_month_is_omitted_while_year_day_and_nested_month_remain() {
    let start = utc_unix(2023, 12, 1, 0, 0, 0);
    let january = utc_unix(2024, 1, 1, 0, 0, 0);
    let cutoff = utc_unix(2024, 1, 10, 0, 0, 0);
    let payload = serde_json::json!({
        "jsonversion": 2,
        "interfaces": [{
            "name": "eth0",
            "created": {"timestamp": start},
            "updated": {"timestamp": cutoff},
            "traffic": {
                "day": [
                    {"timestamp": january, "rx": 100, "tx": 50}
                ],
                "month": [
                    {"timestamp": start, "rx": 9_999, "tx": 4_999},
                    {"timestamp": january, "rx": 300, "tx": 150}
                ],
                "year": [
                    {"timestamp": utc_unix(2023, 1, 1, 0, 0, 0), "rx": 3_100, "tx": 1_550},
                    {"timestamp": january, "rx": 900, "tx": 450}
                ]
            }
        }]
    });
    let config = VnstatCalendarConfig {
        month_rotate: 7,
        month_rotate_affects_years: false,
        use_utc: true,
        trafficless_entries: true,
    };

    let (_, buckets) = parse_vnstat_payload(&payload, "eth0", start, &config).unwrap();

    assert!(!buckets.iter().any(|bucket| bucket.rx_bytes == 9_999));
    assert!(buckets.iter().any(|bucket| bucket.rx_bytes == 3_100));
    assert!(buckets.iter().any(|bucket| bucket.rx_bytes == 900));
    assert!(buckets.iter().any(|bucket| bucket.rx_bytes == 100));
    assert!(buckets.iter().any(|bucket| {
        bucket.rx_bytes == 300 && bucket.start_unix == utc_unix(2024, 1, 7, 0, 0, 0)
    }));
}

#[test]
fn crossing_rotated_month_remains_when_yearly_collection_is_disabled() {
    let label = utc_unix(2023, 12, 1, 0, 0, 0);
    let period_start = utc_unix(2023, 12, 7, 0, 0, 0);
    let cutoff = utc_unix(2024, 1, 10, 0, 0, 0);
    let payload = serde_json::json!({
        "jsonversion": 2,
        "interfaces": [{
            "name": "eth0",
            "created": {"timestamp": period_start},
            "updated": {"timestamp": cutoff},
            "traffic": {
                "month": [
                    {"timestamp": label, "rx": 3_100, "tx": 1_550}
                ]
            }
        }]
    });
    let config = VnstatCalendarConfig {
        month_rotate: 7,
        month_rotate_affects_years: false,
        use_utc: true,
        trafficless_entries: true,
    };

    let (_, buckets) = parse_vnstat_payload(&payload, "eth0", period_start, &config).unwrap();

    assert_eq!(buckets.len(), 1);
    assert_eq!(buckets[0].start_unix, period_start);
    assert_eq!(
        buckets[0].start_unix + u64::from(buckets[0].duration_secs),
        utc_unix(2024, 1, 7, 0, 0, 0)
    );
    assert_eq!(buckets[0].rx_bytes, 3_100);
}

#[test]
fn crossing_rotated_month_rejects_partial_year_coverage() {
    let label = utc_unix(2023, 12, 1, 0, 0, 0);
    let period_start = utc_unix(2023, 12, 7, 0, 0, 0);
    let january = utc_unix(2024, 1, 1, 0, 0, 0);
    let cutoff = utc_unix(2024, 1, 10, 0, 0, 0);
    let payload = serde_json::json!({
        "jsonversion": 2,
        "interfaces": [{
            "name": "eth0",
            "created": {"timestamp": period_start},
            "updated": {"timestamp": cutoff},
            "traffic": {
                "month": [
                    {"timestamp": label, "rx": 3_100, "tx": 1_550}
                ],
                "year": [
                    {"timestamp": january, "rx": 700, "tx": 350}
                ]
            }
        }]
    });
    let config = VnstatCalendarConfig {
        month_rotate: 7,
        month_rotate_affects_years: false,
        use_utc: true,
        trafficless_entries: true,
    };

    let error = parse_vnstat_payload(&payload, "eth0", period_start, &config)
        .unwrap_err()
        .to_string();

    assert!(error.contains("partially cover a rotated month"));
}

#[test]
fn sparse_day_uses_its_natural_next_calendar_midnight() {
    let start = utc_unix(2024, 3, 10, 0, 0, 0);
    let end = utc_unix(2024, 3, 11, 0, 0, 0);
    let payload = serde_json::json!({
        "jsonversion": 2,
        "interfaces": [{
            "name": "eth0",
            "created": {"timestamp": start},
            "updated": {"timestamp": end},
            "traffic": {
                "day": [
                    {"timestamp": start, "rx": 1, "tx": 2}
                ]
            }
        }]
    });

    let (_, buckets) =
        parse_vnstat_payload(&payload, "eth0", start, &utc_calendar_config()).unwrap();
    assert_eq!(buckets.len(), 1);
    assert_eq!(buckets[0].duration_secs, 86_400);
}

#[test]
fn calendar_day_end_handles_sparse_dst_days_without_an_adjacent_row() {
    use chrono_tz::America::New_York;

    let spring_start = u64::try_from(
        New_York
            .with_ymd_and_hms(2024, 3, 10, 0, 0, 0)
            .single()
            .unwrap()
            .timestamp(),
    )
    .unwrap();
    let spring_end = u64::try_from(
        New_York
            .with_ymd_and_hms(2024, 3, 11, 0, 0, 0)
            .single()
            .unwrap()
            .timestamp(),
    )
    .unwrap();
    let fall_start = u64::try_from(
        New_York
            .with_ymd_and_hms(2024, 11, 3, 0, 0, 0)
            .single()
            .unwrap()
            .timestamp(),
    )
    .unwrap();
    let fall_end = u64::try_from(
        New_York
            .with_ymd_and_hms(2024, 11, 4, 0, 0, 0)
            .single()
            .unwrap()
            .timestamp(),
    )
    .unwrap();

    assert_eq!(
        calendar_day_end_unix_in_timezone(spring_start, &New_York).unwrap(),
        spring_end
    );
    assert_eq!(spring_end - spring_start, 23 * 60 * 60);
    assert_eq!(
        calendar_day_end_unix_in_timezone(fall_start, &New_York).unwrap(),
        fall_end
    );
    assert_eq!(fall_end - fall_start, 25 * 60 * 60);
}

#[test]
fn identical_current_calendar_intervals_are_emitted_once() {
    let current_start = 1_704_067_200_u64;
    let cutoff = current_start + 3_600;
    let row = serde_json::json!({
        "timestamp": current_start,
        "rx": 3_600,
        "tx": 1_800
    });
    let payload = serde_json::json!({
        "jsonversion": 2,
        "interfaces": [{
            "name": "eth0",
            "created": {"timestamp": current_start - 60},
            "updated": {"timestamp": cutoff},
            "traffic": {
                "hour": [row.clone()],
                "day": [row.clone()],
                "month": [row.clone()],
                "year": [row]
            }
        }]
    });

    let (_, buckets) =
        parse_vnstat_payload(&payload, "eth0", current_start, &utc_calendar_config()).unwrap();

    assert_eq!(buckets.len(), 1);
    assert_eq!(buckets[0].start_unix, current_start);
    assert_eq!(buckets[0].duration_secs, 3_600);
    assert_eq!(buckets[0].rx_bytes, 3_600);
    assert_eq!(buckets[0].tx_bytes, 1_800);
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
fn request_accepts_a_start_older_than_thirty_five_days() {
    let now = 1_722_474_037;
    let ninety_days_ago = floor_minute(now) - 90 * 24 * 60 * 60;

    assert!(validate_request_at(&["eth0".to_string()], ninety_days_ago, now).is_ok());
}

#[test]
fn parser_rejects_non_v2_json() {
    let payload = serde_json::json!({"jsonversion": "1", "interfaces": []});
    assert!(parse_vnstat_payload(
        &payload,
        "eth0",
        1_722_470_400,
        &VnstatCalendarConfig::default(),
    )
    .is_err());
}
