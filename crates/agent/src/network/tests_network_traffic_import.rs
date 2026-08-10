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

fn local_unix(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> u64 {
    u64::try_from(
        Local
            .with_ymd_and_hms(year, month, day, hour, minute, 0)
            .single()
            .unwrap()
            .timestamp(),
    )
    .unwrap()
}

const VNSTAT_2_0_TIMESTAMP_FREE_JSON: &str = r#"{
  "vnstatversion": "2.0",
  "jsonversion": "2",
  "interfaces": [{
    "name": "eth0",
    "alias": "",
    "created": {"date": {"year": 2024, "month": 1, "day": 15}},
    "updated": {
      "date": {"year": 2024, "month": 4, "day": 2},
      "time": {"hour": 0, "minute": 0}
    },
    "traffic": {
      "total": {"rx": 100000, "tx": 50000},
      "fiveminute": [{
        "id": 1,
        "date": {"year": 2024, "month": 4, "day": 1},
        "time": {"hour": 23, "minute": 50},
        "rx": 305,
        "tx": 105
      }],
      "hour": [{
        "id": 2,
        "date": {"year": 2024, "month": 4, "day": 1},
        "time": {"hour": 22, "minute": 0},
        "rx": 3601,
        "tx": 1801
      }],
      "day": [{
        "id": 3,
        "date": {"year": 2024, "month": 4, "day": 1},
        "rx": 86401,
        "tx": 43201
      }],
      "month": [{
        "id": 4,
        "date": {"year": 2024, "month": 3},
        "rx": 2678401,
        "tx": 1339201
      }],
      "year": [{
        "id": 5,
        "date": {"year": 2024},
        "rx": 8000001,
        "tx": 4000001
      }]
    }
  }]
}"#;

const VNSTAT_2_9_TIMESTAMP_FREE_JSON: &str = r#"{
  "vnstatversion": "2.9",
  "jsonversion": "2",
  "interfaces": [{
    "name": "ens3",
    "alias": "uplink",
    "created": {"date": {"year": 2024, "month": 1, "day": 15}},
    "updated": {
      "date": {"year": 2024, "month": 4, "day": 2},
      "time": {"hour": 0, "minute": 0}
    },
    "traffic": {
      "total": {"rx": 200000, "tx": 100000},
      "fiveminute": [{
        "id": 11,
        "date": {"year": 2024, "month": 4, "day": 1},
        "time": {"hour": 23, "minute": 50},
        "rx": 306,
        "tx": 106
      }],
      "hour": [{
        "id": 12,
        "date": {"year": 2024, "month": 4, "day": 1},
        "time": {"hour": 22, "minute": 0},
        "rx": 3602,
        "tx": 1802
      }],
      "day": [{
        "id": 13,
        "date": {"year": 2024, "month": 4, "day": 1},
        "rx": 86402,
        "tx": 43202
      }],
      "month": [{
        "id": 14,
        "date": {"year": 2024, "month": 3},
        "rx": 2678402,
        "tx": 1339202
      }],
      "year": [{
        "id": 15,
        "date": {"year": 2024},
        "rx": 8000002,
        "tx": 4000002
      }]
    }
  }]
}"#;

fn vnstat_version(minor: u32) -> VnstatVersion {
    VnstatVersion { major: 2, minor }
}

#[test]
fn vnstat_query_uses_the_supported_iface_flag() {
    let command = vnstat_query_command("/usr/bin/vnstat", vnstat_version(13), Some("eth0"));
    let args = command
        .as_std()
        .get_args()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect::<Vec<_>>();

    assert_eq!(args, ["--json", "--limit", "0", "--iface", "eth0"]);
    assert!(!args.iter().any(|argument| argument == "--interface"));
}

#[test]
fn vnstat_query_selects_arguments_supported_by_each_v2_release_family() {
    for minor in [0, 5] {
        let command = vnstat_query_command("/usr/bin/vnstat", vnstat_version(minor), Some("eth0"));
        let args = command
            .as_std()
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(args, ["--json", "0", "--iface", "eth0"]);
    }

    let legacy_all = vnstat_query_command("/usr/bin/vnstat", vnstat_version(0), None);
    let legacy_all_args = legacy_all
        .as_std()
        .get_args()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert_eq!(legacy_all_args, ["--json", "0"]);

    for minor in [6, 9, 10, 13] {
        let command = vnstat_query_command("/usr/bin/vnstat", vnstat_version(minor), Some("eth0"));
        let args = command
            .as_std()
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(args, ["--json", "--limit", "0", "--iface", "eth0"]);
    }
}

#[test]
fn parses_supported_vnstat_versions_and_rejects_v1() {
    assert_eq!(
        parse_vnstat_version("vnStat 2.0 by Teemu Toivola\n").unwrap(),
        vnstat_version(0)
    );
    assert_eq!(
        parse_vnstat_version("vnStat 2.9.1 by Teemu Toivola\n").unwrap(),
        vnstat_version(9)
    );
    assert_eq!(
        parse_vnstat_version("vnStat 2.13\nCopyright elsewhere\n").unwrap(),
        vnstat_version(13)
    );
    assert!(parse_vnstat_version("vnStat 1.18 by Teemu Toivola\n").is_err());
    assert!(parse_vnstat_version("vnStat unknown\n").is_err());
}

#[test]
fn vnstat_version_query_uses_the_portable_version_flag() {
    let command = vnstat_version_command("/usr/bin/vnstat");
    let args = command
        .as_std()
        .get_args()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect::<Vec<_>>();

    assert_eq!(args, ["--version"]);
}

#[test]
fn vnstat_all_interface_query_omits_iface_and_discovery_is_bounded_and_deduplicated() {
    let command = vnstat_query_command("/usr/bin/vnstat", vnstat_version(13), None);
    let args = command
        .as_std()
        .get_args()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert_eq!(args, ["--json", "--limit", "0"]);

    let payload = serde_json::json!({
        "jsonversion": 2,
        "interfaces": [
            {"name": "ens3"},
            {"name": "eth0"},
            {"name": "ens3"}
        ]
    });
    assert_eq!(
        discover_vnstat_interfaces(&payload).unwrap(),
        ["ens3".to_string(), "eth0".to_string()]
    );

    let invalid = serde_json::json!({
        "jsonversion": 2,
        "interfaces": [{"name": "../eth0"}]
    });
    assert!(discover_vnstat_interfaces(&invalid).is_err());
}

#[test]
fn all_interface_snapshot_parses_every_discovered_source_and_bucket() {
    let start = 1_722_470_400_u64;
    let payload = serde_json::json!({
        "jsonversion": 2,
        "interfaces": [
            {
                "name": "eth0",
                "created": {"timestamp": start},
                "updated": {"timestamp": start + 600},
                "traffic": {
                    "fiveminute": [
                        {"timestamp": start, "rx": 100, "tx": 50}
                    ]
                }
            },
            {
                "name": "ens3",
                "created": {"timestamp": start + 60},
                "updated": {"timestamp": start + 660},
                "traffic": {
                    "fiveminute": [
                        {"timestamp": start + 60, "rx": 200, "tx": 75}
                    ]
                }
            }
        ]
    });

    let (result_interfaces, sources, buckets) =
        parse_discovered_vnstat_payload(&payload, start, &utc_calendar_config()).unwrap();

    assert_eq!(result_interfaces, ["ens3".to_string(), "eth0".to_string()]);
    assert_eq!(
        sources
            .iter()
            .map(|source| source.interface.as_str())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["ens3", "eth0"])
    );
    assert_eq!(
        buckets
            .iter()
            .map(|bucket| bucket.interface.as_str())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["ens3", "eth0"])
    );
    assert!(buckets
        .iter()
        .any(|bucket| bucket.interface == "eth0" && bucket.rx_bytes == 100));
    assert!(buckets
        .iter()
        .any(|bucket| bucket.interface == "ens3" && bucket.rx_bytes == 200));
}

#[test]
fn empty_requested_interfaces_are_valid_for_vnstat_discovery() {
    assert!(validate_request_at(&[], 60, 180).is_ok());
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
fn parses_realistic_vnstat_2_0_timestamp_free_json_at_local_calendar_boundaries() {
    let payload: Value = serde_json::from_str(VNSTAT_2_0_TIMESTAMP_FREE_JSON).unwrap();
    let created = local_unix(2024, 1, 15, 0, 0);
    let updated = local_unix(2024, 4, 2, 0, 0);

    let (source, buckets) = parse_vnstat_payload_for_version(
        &payload,
        "eth0",
        created,
        &VnstatCalendarConfig::default(),
        vnstat_version(0),
    )
    .unwrap();

    assert_eq!(source.database_created_unix, Some(created));
    assert_eq!(source.source_updated_unix, Some(updated));
    assert!(buckets.iter().any(|bucket| {
        bucket.rx_bytes == 305 && bucket.start_unix == local_unix(2024, 4, 1, 23, 50)
    }));
    assert!(buckets.iter().any(|bucket| {
        bucket.rx_bytes == 3_601 && bucket.start_unix == local_unix(2024, 4, 1, 22, 0)
    }));
    assert!(buckets.iter().any(|bucket| {
        bucket.rx_bytes == 86_401 && bucket.start_unix == local_unix(2024, 4, 1, 0, 0)
    }));
    assert!(buckets.iter().any(|bucket| {
        bucket.rx_bytes == 2_678_401 && bucket.start_unix == local_unix(2024, 3, 1, 0, 0)
    }));
    assert!(buckets
        .iter()
        .any(|bucket| { bucket.rx_bytes == 8_000_001 && bucket.start_unix == created }));
}

#[test]
fn parses_realistic_vnstat_2_9_timestamp_free_json_with_rotated_utc_periods() {
    let payload: Value = serde_json::from_str(VNSTAT_2_9_TIMESTAMP_FREE_JSON).unwrap();
    let created = utc_unix(2024, 1, 15, 0, 0, 0);
    let updated = utc_unix(2024, 4, 2, 0, 0, 0);
    let config = VnstatCalendarConfig {
        month_rotate: 7,
        month_rotate_affects_years: true,
        use_utc: true,
        trafficless_entries: true,
    };

    let (source, buckets) =
        parse_vnstat_payload_for_version(&payload, "ens3", created, &config, vnstat_version(9))
            .unwrap();

    assert_eq!(source.database_created_unix, Some(created));
    assert_eq!(source.source_updated_unix, Some(updated));
    assert!(buckets.iter().any(|bucket| {
        bucket.rx_bytes == 306 && bucket.start_unix == utc_unix(2024, 4, 1, 23, 50, 0)
    }));
    assert!(buckets.iter().any(|bucket| {
        bucket.rx_bytes == 3_602 && bucket.start_unix == utc_unix(2024, 4, 1, 22, 0, 0)
    }));
    assert!(buckets.iter().any(|bucket| {
        bucket.rx_bytes == 86_402 && bucket.start_unix == utc_unix(2024, 4, 1, 0, 0, 0)
    }));
    assert!(buckets.iter().any(|bucket| {
        bucket.rx_bytes == 2_678_402 && bucket.start_unix == utc_unix(2024, 3, 7, 0, 0, 0)
    }));
    assert!(buckets
        .iter()
        .any(|bucket| { bucket.rx_bytes == 8_000_002 && bucket.start_unix == created }));
}

#[test]
fn numeric_timestamps_take_precedence_over_legacy_calendar_fields() {
    let created = utc_unix(2024, 1, 15, 12, 0, 31);
    let available = ceil_minute(created).unwrap();
    let row_start = utc_unix(2024, 1, 15, 12, 50, 0);
    let updated = utc_unix(2024, 1, 15, 13, 0, 0);
    let payload = serde_json::json!({
        "vnstatversion": "2.10",
        "jsonversion": "2",
        "interfaces": [{
            "name": "eth0",
            "created": {
                "date": {"year": 2000, "month": 1, "day": 1},
                "timestamp": created
            },
            "updated": {
                "date": {"year": 2000, "month": 1, "day": 1},
                "time": {"hour": 0, "minute": 0},
                "timestamp": updated
            },
            "traffic": {
                "fiveminute": [{
                    "date": {"year": 2000, "month": 1, "day": 1},
                    "time": {"hour": 0, "minute": 0},
                    "timestamp": row_start,
                    "rx": 500,
                    "tx": 250
                }]
            }
        }]
    });

    let (source, buckets) = parse_vnstat_payload_for_version(
        &payload,
        "eth0",
        available,
        &utc_calendar_config(),
        vnstat_version(10),
    )
    .unwrap();

    assert_eq!(source.database_created_unix, Some(created));
    assert_eq!(source.source_updated_unix, Some(updated));
    assert_eq!(buckets.len(), 1);
    assert_eq!(buckets[0].start_unix, row_start);
}

#[test]
fn vnstat_2_10_and_newer_reject_timestamp_free_json() {
    let payload: Value = serde_json::from_str(VNSTAT_2_9_TIMESTAMP_FREE_JSON).unwrap();
    let error = parse_vnstat_payload_for_version(
        &payload,
        "ens3",
        utc_unix(2024, 1, 15, 0, 0, 0),
        &utc_calendar_config(),
        vnstat_version(10),
    )
    .unwrap_err()
    .to_string();

    assert!(error.contains("missing its timestamp"));
}

#[test]
fn malformed_present_timestamp_does_not_fall_back_to_legacy_calendar_fields() {
    let payload = serde_json::json!({
        "vnstatversion": "2.9",
        "jsonversion": "2",
        "interfaces": [{
            "name": "eth0",
            "created": {
                "date": {"year": 2024, "month": 1, "day": 15},
                "timestamp": "1705276800"
            },
            "updated": {
                "date": {"year": 2024, "month": 1, "day": 16},
                "time": {"hour": 0, "minute": 0}
            },
            "traffic": {
                "day": [{
                    "date": {"year": 2024, "month": 1, "day": 15},
                    "rx": 1,
                    "tx": 2
                }]
            }
        }]
    });

    assert!(parse_vnstat_payload_for_version(
        &payload,
        "eth0",
        utc_unix(2024, 1, 15, 0, 0, 0),
        &utc_calendar_config(),
        vnstat_version(9),
    )
    .is_err());
}

#[test]
fn legacy_calendar_parser_rejects_invalid_dates() {
    let mut payload: Value = serde_json::from_str(VNSTAT_2_0_TIMESTAMP_FREE_JSON).unwrap();
    payload["interfaces"][0]["traffic"]["day"][0]["date"] =
        serde_json::json!({"year": 2024, "month": 2, "day": 30});
    let created = local_unix(2024, 1, 15, 0, 0);

    assert!(parse_vnstat_payload_for_version(
        &payload,
        "eth0",
        created,
        &VnstatCalendarConfig::default(),
        vnstat_version(0),
    )
    .is_err());
}

#[test]
fn legacy_local_minutes_reject_dst_gaps_and_overlaps() {
    use chrono_tz::America::New_York;

    let spring_gap = serde_json::json!({
        "date": {"year": 2024, "month": 3, "day": 10},
        "time": {"hour": 2, "minute": 30}
    });
    let fall_overlap = serde_json::json!({
        "date": {"year": 2024, "month": 11, "day": 3},
        "time": {"hour": 1, "minute": 30}
    });

    assert!(legacy_minute_unix_in_timezone(&spring_gap, &New_York, "test row").is_err());
    assert!(legacy_minute_unix_in_timezone(&fall_overlap, &New_York, "test row").is_err());
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
    assert_eq!(source.retained_start_unix, 1_722_000_000);
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

    let (source, buckets) =
        parse_vnstat_payload(&payload, "eth0", database_created, &config).unwrap();

    assert_eq!(source.retained_start_unix, first_retained_year);
    assert!(buckets
        .iter()
        .all(|bucket| bucket.start_unix >= first_retained_year));
}

#[test]
fn retained_start_skips_an_older_component_separated_by_a_later_gap() {
    let year_2020 = utc_unix(2020, 1, 1, 0, 0, 0);
    let year_2022 = utc_unix(2022, 1, 1, 0, 0, 0);
    let year_2023 = utc_unix(2023, 1, 1, 0, 0, 0);
    let cutoff = utc_unix(2024, 1, 1, 0, 0, 0);
    let payload = serde_json::json!({
        "jsonversion": 2,
        "interfaces": [{
            "name": "eth0",
            "created": {"timestamp": year_2020},
            "updated": {"timestamp": cutoff},
            "traffic": {
                "year": [
                    {"timestamp": year_2020, "rx": 365, "tx": 100},
                    {"timestamp": year_2022, "rx": 365, "tx": 100},
                    {"timestamp": year_2023, "rx": 730, "tx": 200}
                ]
            }
        }]
    });

    let (source, buckets) =
        parse_vnstat_payload(&payload, "eth0", year_2020, &utc_calendar_config()).unwrap();

    assert!(buckets.iter().any(|bucket| bucket.start_unix == year_2020));
    assert!(buckets.iter().any(|bucket| bucket.start_unix == year_2022));
    assert_eq!(source.retained_start_unix, year_2022);
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
