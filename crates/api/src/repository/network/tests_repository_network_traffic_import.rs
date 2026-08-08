use super::*;

fn bucket(
    start_unix: u64,
    duration_secs: u32,
    rx_bytes: u64,
    tx_bytes: u64,
) -> NetworkTrafficImportBucket {
    NetworkTrafficImportBucket {
        interface: "eth0".to_string(),
        start_unix,
        duration_secs,
        rx_bytes,
        tx_bytes,
    }
}

fn minute_rows(traffic: &ExpandedMinuteTraffic) -> Vec<(u64, u64, u64)> {
    traffic
        .segments
        .iter()
        .flat_map(|segment| {
            (segment.start_unix..segment.end_unix)
                .step_by(60)
                .map(|observed_unix| (observed_unix, segment.rx_bytes, segment.tx_bytes))
        })
        .collect()
}

#[test]
fn finer_rows_are_preserved_and_only_coarse_residual_is_distributed() {
    let start = 1_722_470_400;
    let end = start + 3_600;
    let buckets = vec![
        bucket(start, 3_600, 3_600, 1_800),
        bucket(start, 300, 1_000, 500),
    ];

    let traffic = expand_buckets_to_minutes(&buckets, "eth0", start, end).unwrap();
    let minutes = minute_rows(&traffic);

    assert_eq!(minutes.len(), 60);
    assert_eq!(traffic.total_rx_bytes, 3_600);
    assert_eq!(traffic.total_tx_bytes, 1_800);
    assert_eq!(minutes.iter().take(5).map(|row| row.1).sum::<u64>(), 1_000);
    assert_eq!(minutes.iter().take(5).map(|row| row.2).sum::<u64>(), 500);
}

#[test]
fn ancient_daily_history_uses_bounded_segments_without_a_lookback_rejection() {
    let start = 60;
    let days = 90_u64;
    let end = start + days * 86_400;
    let buckets = (0..days)
        .map(|day| bucket(start + day * 86_400, 86_400, 86_400, 43_200))
        .collect::<Vec<_>>();

    let traffic = expand_buckets_to_minutes(&buckets, "eth0", start, end).unwrap();

    assert_eq!(traffic.minute_count, days * 24 * 60);
    assert_eq!(traffic.total_rx_bytes, days * 86_400);
    assert_eq!(traffic.total_tx_bytes, days * 43_200);
    assert_eq!(traffic.segments.len(), 1);
}

#[test]
fn explicit_trafficless_year_keeps_ancient_import_continuous_and_bounded() {
    let start = 1_609_459_200; // 2021-01-01T00:00:00Z
    let year_2022 = 1_640_995_200;
    let year_2023 = 1_672_531_200;
    let live = 1_704_067_200; // 2024-01-01T00:00:00Z
    let job_id = Uuid::parse_str("33333333-3333-4333-8333-333333333333").unwrap();
    let result = NetworkTrafficImportResult {
        r#type: "network_traffic_import_vnstat".to_string(),
        status: "collected".to_string(),
        requested_start_unix: start,
        collected_until_unix: live,
        interfaces: vec!["eth0".to_string()],
        sources: vec![vpsman_common::NetworkTrafficImportSource {
            interface: "eth0".to_string(),
            database_created_unix: Some(start),
            source_updated_unix: Some(live),
        }],
        batch_count: 1,
        bucket_count: 3,
        message: String::new(),
    };
    let buckets = vec![
        bucket(start, 365 * 86_400, 365, 100),
        bucket(year_2022, 365 * 86_400, 0, 0),
        bucket(year_2023, 365 * 86_400, 730, 200),
    ];
    let existing =
        vec![sample_record("agent-a", "eth0", live, 10, 20, "interface_counters").unwrap()];

    let prepared = prepare_imports(
        job_id,
        "agent-a",
        &["eth0".to_string()],
        start,
        &result,
        &buckets,
        live + 60,
        &existing,
    )
    .unwrap();

    assert_eq!(prepared.len(), 1);
    assert_eq!(prepared[0].traffic.minute_count, (live - start) / 60);
    assert_eq!(prepared[0].imported_rx_bytes, 1_095);
    assert_eq!(prepared[0].imported_tx_bytes, 300);
    assert_eq!(
        prepared[0].traffic.segments.first().unwrap().start_unix,
        start
    );
    assert_eq!(prepared[0].traffic.segments.last().unwrap().end_unix, live);
    assert!(prepared[0].traffic.segments.iter().any(|segment| {
        segment.start_unix <= year_2022
            && segment.end_unix >= year_2023
            && segment.rx_bytes == 0
            && segment.tx_bytes == 0
    }));
    assert!(prepared[0].traffic.segments.len() <= 6);
}

#[test]
fn sparse_month_buckets_emitted_by_agent_expand_through_known_zero_gap() {
    let january = 1_672_531_200; // 2023-01-01T00:00:00Z
    let february = 1_675_209_600;
    let march = 1_677_628_800;
    let april = 1_680_307_200;
    // Exact agent output for Jan + missing trafficless Feb + Mar.
    let buckets = vec![
        bucket(january, 31 * 86_400, 31, 10),
        bucket(february, 28 * 86_400, 0, 0),
        bucket(march, 31 * 86_400, 31, 20),
    ];

    let traffic = expand_buckets_to_minutes(&buckets, "eth0", january, april).unwrap();

    assert_eq!(traffic.minute_count, 90 * 24 * 60);
    assert_eq!(traffic.total_rx_bytes, 62);
    assert_eq!(traffic.total_tx_bytes, 30);
    assert!(traffic.segments.iter().any(|segment| {
        segment.start_unix <= february
            && segment.end_unix >= march
            && segment.rx_bytes == 0
            && segment.tx_bytes == 0
    }));
}

#[test]
fn calendar_month_and_leap_year_buckets_are_accepted() {
    let start = 1_704_067_200;
    let month_end = start + 31 * 86_400;
    let month = expand_buckets_to_minutes(
        &[bucket(start, 31 * 86_400, 31_000, 15_500)],
        "eth0",
        start,
        month_end,
    )
    .unwrap();
    assert_eq!(month.minute_count, 31 * 24 * 60);

    let leap_year_end = start + 366 * 86_400;
    let year = expand_buckets_to_minutes(
        &[bucket(start, 366 * 86_400, 366_000, 183_000)],
        "eth0",
        start,
        leap_year_end,
    )
    .unwrap();
    assert_eq!(year.minute_count, 366 * 24 * 60);
}

#[test]
fn aligned_year_day_and_rotated_month_coverage_preserves_each_aggregate() {
    let december_start = 1_701_388_800; // 2023-12-01T00:00:00Z
    let january_start = 1_704_067_200; // 2024-01-01T00:00:00Z
    let january_second = january_start + 86_400;
    let rotated_month_start = january_start + 6 * 86_400;
    let end = january_start + 9 * 86_400;
    let buckets = vec![
        bucket(
            december_start,
            u32::try_from(january_start - december_start).unwrap(),
            3_100,
            1_550,
        ),
        bucket(
            january_start,
            u32::try_from(end - january_start).unwrap(),
            900,
            450,
        ),
        bucket(january_start, 86_400, 100, 50),
        bucket(
            rotated_month_start,
            u32::try_from(end - rotated_month_start).unwrap(),
            300,
            150,
        ),
    ];

    let traffic = expand_buckets_to_minutes(&buckets, "eth0", december_start, end).unwrap();
    let minutes = minute_rows(&traffic);
    let range_total = |start, finish, direction: usize| {
        minutes
            .iter()
            .filter(|row| row.0 >= start && row.0 < finish)
            .map(|row| if direction == 1 { row.1 } else { row.2 })
            .sum::<u64>()
    };

    assert_eq!(traffic.total_rx_bytes, 4_000);
    assert_eq!(traffic.total_tx_bytes, 2_000);
    assert_eq!(range_total(december_start, january_start, 1), 3_100);
    assert_eq!(range_total(january_start, january_second, 1), 100);
    assert_eq!(range_total(rotated_month_start, end, 1), 300);
    assert_eq!(range_total(january_start, end, 1), 900);
}

#[test]
fn fully_covered_inconsistent_coarse_row_is_rejected() {
    let start = 1_722_470_400;
    let buckets = vec![
        bucket(start, 600, 120, 30),
        bucket(start, 300, 50, 10),
        bucket(start + 300, 300, 50, 10),
    ];

    let error = expand_buckets_to_minutes(&buckets, "eth0", start, start + 600)
        .unwrap_err()
        .to_string();
    assert!(error.contains("fully_covered_bucket_total_mismatch"));
}

#[test]
fn gaps_in_retained_history_are_rejected() {
    let start = 1_722_470_400;
    let buckets = vec![bucket(start, 300, 50, 10), bucket(start + 600, 300, 50, 10)];

    let error = expand_buckets_to_minutes(&buckets, "eth0", start, start + 900)
        .unwrap_err()
        .to_string();
    assert!(error.contains("vnstat_history_gap"));
}

#[test]
fn import_to_live_boundary_is_intentional_but_reverse_is_not() {
    assert!(is_intentional_vnstat_import_boundary(
        "vnstat_import:11111111-1111-4111-8111-111111111111",
        "interface_counters",
    ));
    assert!(!is_intentional_vnstat_import_boundary(
        "interface_counters",
        "vnstat_import:11111111-1111-4111-8111-111111111111",
    ));
}

#[test]
fn prepare_import_stops_immediately_before_first_live_sample() {
    let start = 1_722_470_400;
    let live = start + 600;
    let job_id = Uuid::parse_str("11111111-1111-4111-8111-111111111111").unwrap();
    let result = NetworkTrafficImportResult {
        r#type: "network_traffic_import_vnstat".to_string(),
        status: "collected".to_string(),
        requested_start_unix: start,
        collected_until_unix: live + 600,
        interfaces: vec!["eth0".to_string()],
        sources: vec![vpsman_common::NetworkTrafficImportSource {
            interface: "eth0".to_string(),
            database_created_unix: Some(start - 60),
            source_updated_unix: Some(live + 600),
        }],
        batch_count: 1,
        bucket_count: 1,
        message: String::new(),
    };
    let existing =
        vec![sample_record("agent-a", "eth0", live, 10, 20, "interface_counters").unwrap()];
    let prepared = prepare_imports(
        job_id,
        "agent-a",
        &["eth0".to_string()],
        start,
        &result,
        &[bucket(start, 600, 100, 50)],
        live + 600,
        &existing,
    )
    .unwrap();
    let samples = prepared[0]
        .samples("agent-a")
        .collect::<Result<Vec<_>>>()
        .unwrap();

    assert_eq!(prepared[0].end_unix, live);
    assert_eq!(samples.first().unwrap().observed_unix, (start - 60) as i64);
    assert_eq!(samples.last().unwrap().observed_unix, (live - 60) as i64);
}

#[test]
fn rerun_boundaries_prepare_the_same_replacement_as_full_import_history() {
    let start = 1_722_470_400;
    let live = start + 600;
    let job_id = Uuid::parse_str("22222222-2222-4222-8222-222222222222").unwrap();
    let result = NetworkTrafficImportResult {
        r#type: "network_traffic_import_vnstat".to_string(),
        status: "collected".to_string(),
        requested_start_unix: start,
        collected_until_unix: live + 600,
        interfaces: vec!["eth0".to_string()],
        sources: vec![vpsman_common::NetworkTrafficImportSource {
            interface: "eth0".to_string(),
            database_created_unix: Some(start - 60),
            source_updated_unix: Some(live + 600),
        }],
        batch_count: 1,
        bucket_count: 1,
        message: String::new(),
    };
    let previous = sample_record(
        "agent-a",
        "eth0",
        start - 60,
        700,
        900,
        "interface_counters",
    )
    .unwrap();
    let first_live = sample_record("agent-a", "eth0", live, 20, 30, "interface_counters").unwrap();
    let boundaries = vec![previous.clone(), first_live.clone()];
    let mut full_history = boundaries.clone();
    for minute in 0..10 {
        full_history.push(
            sample_record(
                "agent-a",
                "eth0",
                start + minute * 60,
                800 + i64::try_from(minute).unwrap(),
                1_000 + i64::try_from(minute).unwrap(),
                "vnstat_import:11111111-1111-4111-8111-111111111111",
            )
            .unwrap(),
        );
    }

    let prepare = |existing: &[TrafficCounterSampleRecord]| {
        prepare_imports(
            job_id,
            "agent-a",
            &["eth0".to_string()],
            start,
            &result,
            &[bucket(start, 600, 100, 50)],
            live + 600,
            existing,
        )
        .unwrap()
    };
    let from_full_history = prepare(&full_history);
    let from_boundaries = prepare(&boundaries);
    let sample_values = |prepared: &PreparedInterfaceImport| {
        prepared
            .samples("agent-a")
            .map(|sample| {
                let sample = sample.unwrap();
                (
                    sample.observed_unix,
                    sample.rx_bytes,
                    sample.tx_bytes,
                    sample.sample_source,
                )
            })
            .collect::<Vec<_>>()
    };

    assert_eq!(
        sample_values(&from_full_history[0]),
        sample_values(&from_boundaries[0])
    );
    assert_eq!(
        from_full_history[0].imported_rx_bytes,
        from_boundaries[0].imported_rx_bytes
    );
    assert_eq!(
        from_full_history[0].imported_tx_bytes,
        from_boundaries[0].imported_tx_bytes
    );
}
