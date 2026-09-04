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

fn interface_bucket(
    interface: &str,
    start_unix: u64,
    duration_secs: u32,
    rx_bytes: u64,
    tx_bytes: u64,
) -> NetworkTrafficImportBucket {
    NetworkTrafficImportBucket {
        interface: interface.to_string(),
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
            retained_start_unix: start,
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
            retained_start_unix: start,
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
fn prepare_import_clamps_each_interface_to_its_distinct_continuous_retained_start() {
    let requested_start = 1_722_470_400;
    let eth0_created = requested_start - 3_600;
    let eth0_effective_start = requested_start + 120;
    let eth0_live = requested_start + 600;
    let ens3_created = requested_start - 7_200;
    let ens3_effective_start = requested_start + 240;
    let ens3_live = requested_start + 900;
    let job_id = Uuid::parse_str("44444444-4444-4444-8444-444444444444").unwrap();
    let result = NetworkTrafficImportResult {
        r#type: "network_traffic_import_vnstat".to_string(),
        status: "collected".to_string(),
        requested_start_unix: requested_start,
        collected_until_unix: ens3_live + 60,
        interfaces: vec!["eth0".to_string(), "ens3".to_string()],
        sources: vec![
            vpsman_common::NetworkTrafficImportSource {
                interface: "eth0".to_string(),
                database_created_unix: Some(eth0_created),
                retained_start_unix: eth0_effective_start,
                source_updated_unix: Some(eth0_live + 60),
            },
            vpsman_common::NetworkTrafficImportSource {
                interface: "ens3".to_string(),
                database_created_unix: Some(ens3_created),
                retained_start_unix: ens3_effective_start,
                source_updated_unix: Some(ens3_live + 60),
            },
        ],
        batch_count: 1,
        bucket_count: 4,
        message: String::new(),
    };
    let existing = vec![
        sample_record("agent-a", "eth0", eth0_live, 10, 20, "interface_counters").unwrap(),
        sample_record("agent-a", "ens3", ens3_live, 30, 40, "interface_counters").unwrap(),
    ];
    let buckets = [
        interface_bucket("eth0", requested_start, 60, 10, 5),
        interface_bucket(
            "eth0",
            eth0_effective_start,
            u32::try_from(eth0_live - eth0_effective_start).unwrap(),
            90,
            45,
        ),
        interface_bucket("ens3", requested_start, 120, 20, 10),
        interface_bucket(
            "ens3",
            ens3_effective_start,
            u32::try_from(ens3_live - ens3_effective_start).unwrap(),
            120,
            60,
        ),
    ];
    assert!(validate_result_contract(
        &["eth0".to_string(), "ens3".to_string()],
        requested_start,
        &result,
        &buckets,
        ens3_live + 120,
    )
    .is_ok());
    let prepared = prepare_imports(
        job_id,
        "agent-a",
        &["eth0".to_string(), "ens3".to_string()],
        requested_start,
        &result,
        &buckets,
        ens3_live + 60,
        &existing,
    )
    .unwrap();

    assert_eq!(result.requested_start_unix, requested_start);
    assert_eq!(prepared[0].start_unix, eth0_effective_start);
    assert_eq!(prepared[0].traffic.minute_count, 8);
    assert_eq!(prepared[1].start_unix, ens3_effective_start);
    assert_eq!(prepared[1].traffic.minute_count, 11);
    assert_eq!(
        prepared[0]
            .samples("agent-a")
            .collect::<Result<Vec<_>>>()
            .unwrap()
            .first()
            .unwrap()
            .observed_unix,
        i64::try_from(eth0_effective_start - 60).unwrap()
    );
    assert_eq!(
        prepared[1]
            .samples("agent-a")
            .collect::<Result<Vec<_>>>()
            .unwrap()
            .first()
            .unwrap()
            .observed_unix,
        i64::try_from(ens3_effective_start - 60).unwrap()
    );
}

#[test]
fn empty_request_accepts_the_agents_bounded_discovered_interface_set() {
    let start = 1_722_470_400;
    let result = NetworkTrafficImportResult {
        r#type: "network_traffic_import_vnstat".to_string(),
        status: "collected".to_string(),
        requested_start_unix: start,
        collected_until_unix: start + 600,
        interfaces: vec!["eth0".to_string()],
        sources: vec![vpsman_common::NetworkTrafficImportSource {
            interface: "eth0".to_string(),
            database_created_unix: Some(start),
            retained_start_unix: start,
            source_updated_unix: Some(start + 600),
        }],
        batch_count: 1,
        bucket_count: 1,
        message: String::new(),
    };
    assert!(validate_result_contract(
        &[],
        start,
        &result,
        &[bucket(start, 600, 100, 50)],
        start + 660,
    )
    .is_ok());
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
            retained_start_unix: start,
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

fn prepared_import_for_test(
    start_unix: u64,
    end_unix: u64,
    initial_rx_bytes: i64,
    initial_tx_bytes: i64,
    include_baseline: bool,
    segments: Vec<MinuteAssignmentSegment>,
) -> PreparedInterfaceImport {
    let (total_rx_bytes, total_tx_bytes) =
        assignment_totals_in_range(&segments, start_unix, end_unix).unwrap();
    PreparedInterfaceImport {
        interface: "eth0".to_string(),
        start_unix,
        end_unix,
        initial_rx_bytes,
        initial_tx_bytes,
        initial_rx_counter_epoch: 0,
        initial_tx_counter_epoch: 0,
        include_baseline,
        import_source: "vnstat_import:55555555-5555-4555-8555-555555555555".to_string(),
        traffic: ExpandedMinuteTraffic {
            segments,
            minute_count: (end_unix - start_unix) / 60,
            total_rx_bytes,
            total_tx_bytes,
        },
        imported_rx_bytes: total_rx_bytes,
        imported_tx_bytes: total_tx_bytes,
    }
}

#[test]
fn import_rollup_fixture_replaces_only_import_owned_rows() {
    let utc_day_start_unix = 1_767_312_000; // 2026-01-01T00:00:00Z
    let raw_cutoff_unix = utc_day_start_unix - TRAFFIC_COUNTER_RAW_RETENTION_DAYS as u64 * 86_400;
    let start_unix = raw_cutoff_unix - 3_600;
    let end_unix = start_unix + 1_800;
    let prepared = prepared_import_for_test(
        start_unix,
        end_unix,
        10,
        20,
        true,
        vec![MinuteAssignmentSegment {
            start_unix,
            end_unix,
            rx_bytes: 3,
            tx_bytes: 2,
        }],
    );

    let rows = prepare_test_import_rollups(
        "client-a",
        std::slice::from_ref(&prepared),
        utc_day_start_unix,
        raw_cutoff_unix,
    )
    .unwrap();
    assert!(!rows.is_empty());
    assert!(rows.iter().all(|row| {
        row.client_id == "client-a"
            && row.source_kind == "host"
            && row.interface == "eth0"
            && row.origin_kind == "vnstat_import"
    }));
    assert_eq!(
        rows.iter().map(|row| row.rx_bytes).sum::<i64>(),
        3 * i64::try_from((end_unix - start_unix) / 60).unwrap()
    );
    assert_eq!(
        rows.iter().map(|row| row.tx_bytes).sum::<i64>(),
        2 * i64::try_from((end_unix - start_unix) / 60).unwrap()
    );

    let mut existing = vec![
        TrafficCounterRollupRecord {
            client_id: "client-a".to_string(),
            source_kind: "host".to_string(),
            interface: "eth0".to_string(),
            origin_kind: "live".to_string(),
            bucket_start: "2026-01-01T00:00:00+00:00".to_string(),
            bucket_start_unix: utc_day_start_unix as i64,
            bucket_secs: 3_600,
            rx_bytes: 7,
            tx_bytes: 11,
            rx_valid_count: 1,
            tx_valid_count: 1,
            any_valid_count: 1,
            rx_reset_count: 0,
            tx_reset_count: 0,
            any_reset_count: 0,
            first_observed_unix: utc_day_start_unix as i64,
            latest_observed_unix: utc_day_start_unix as i64,
        },
        TrafficCounterRollupRecord {
            client_id: "client-a".to_string(),
            source_kind: "host".to_string(),
            interface: "eth0".to_string(),
            origin_kind: "vnstat_import".to_string(),
            bucket_start: "2025-11-30T00:00:00+00:00".to_string(),
            bucket_start_unix: raw_cutoff_unix as i64,
            bucket_secs: 3_600,
            rx_bytes: 99,
            tx_bytes: 99,
            rx_valid_count: 1,
            tx_valid_count: 1,
            any_valid_count: 1,
            rx_reset_count: 0,
            tx_reset_count: 0,
            any_reset_count: 0,
            first_observed_unix: raw_cutoff_unix as i64,
            latest_observed_unix: raw_cutoff_unix as i64,
        },
    ];
    existing.retain(|row| {
        row.client_id != "client-a"
            || row.source_kind != "host"
            || row.interface != "eth0"
            || row.origin_kind != "vnstat_import"
    });
    existing.extend(rows);
    assert_eq!(
        existing
            .iter()
            .filter(|row| row.origin_kind == "live")
            .count(),
        1
    );
    assert!(existing
        .iter()
        .filter(|row| row.origin_kind == "vnstat_import")
        .all(|row| row.rx_bytes != 99));
}

fn expanded_minute_rollup_oracle(
    prepared: &PreparedInterfaceImport,
    utc_day_start_unix: u64,
    raw_cutoff_unix: u64,
) -> Vec<PreparedImportRollup> {
    let mut rollups = std::collections::BTreeMap::new();
    let mut previous = (!prepared.include_baseline)
        .then_some((prepared.initial_rx_bytes, prepared.initial_tx_bytes));
    for sample in prepared.samples("agent-a") {
        let sample = sample.unwrap();
        let observed_unix = u64::try_from(sample.observed_unix).unwrap();
        if observed_unix >= raw_cutoff_unix {
            break;
        }
        let bucket_secs = if observed_unix >= utc_day_start_unix - 91 * 86_400 {
            3_600
        } else if observed_unix >= utc_day_start_unix - 181 * 86_400 {
            10_800
        } else if observed_unix >= utc_day_start_unix - 366 * 86_400 {
            21_600
        } else {
            86_400
        };
        let bucket_secs_u64 = u64::try_from(bucket_secs).unwrap();
        let bucket_start_unix = observed_unix - observed_unix % bucket_secs_u64;
        let (rx_bytes, tx_bytes, rx_valid_count, tx_valid_count, any_valid_count) = previous
            .map_or((0, 0, 0, 0, 0), |(previous_rx, previous_tx)| {
                let rx_bytes = sample.rx_bytes.checked_sub(previous_rx);
                let tx_bytes = sample.tx_bytes.checked_sub(previous_tx);
                (
                    rx_bytes
                        .and_then(|value| u64::try_from(value).ok())
                        .unwrap_or(0),
                    tx_bytes
                        .and_then(|value| u64::try_from(value).ok())
                        .unwrap_or(0),
                    u32::from(rx_bytes.is_some_and(|value| value >= 0)),
                    u32::from(tx_bytes.is_some_and(|value| value >= 0)),
                    u32::from(
                        rx_bytes.is_some_and(|value| value >= 0)
                            || tx_bytes.is_some_and(|value| value >= 0),
                    ),
                )
            });
        let entry = rollups
            .entry((bucket_secs, bucket_start_unix))
            .or_insert_with(|| PreparedImportRollup {
                interface: prepared.interface.clone(),
                bucket_secs,
                bucket_start_unix,
                rx_bytes: 0,
                tx_bytes: 0,
                rx_valid_count: 0,
                tx_valid_count: 0,
                any_valid_count: 0,
                first_observed_unix: observed_unix,
                latest_observed_unix: observed_unix,
            });
        entry.rx_bytes += rx_bytes;
        entry.tx_bytes += tx_bytes;
        entry.rx_valid_count += rx_valid_count;
        entry.tx_valid_count += tx_valid_count;
        entry.any_valid_count += any_valid_count;
        entry.first_observed_unix = entry.first_observed_unix.min(observed_unix);
        entry.latest_observed_unix = entry.latest_observed_unix.max(observed_unix);
        previous = Some((sample.rx_bytes, sample.tx_bytes));
    }
    rollups.into_values().collect()
}

#[test]
fn five_year_import_materializes_exact_bounded_rollups_and_raw_tail() {
    let start_unix = u64::try_from(
        Utc.with_ymd_and_hms(2021, 3, 1, 0, 0, 0)
            .single()
            .unwrap()
            .timestamp(),
    )
    .unwrap();
    let utc_day_start_unix = u64::try_from(
        Utc.with_ymd_and_hms(2026, 3, 1, 0, 0, 0)
            .single()
            .unwrap()
            .timestamp(),
    )
    .unwrap();
    let raw_cutoff_unix = utc_day_start_unix - TRAFFIC_COUNTER_RAW_RETENTION_DAYS as u64 * 86_400;
    let prepared = prepared_import_for_test(
        start_unix,
        utc_day_start_unix,
        0,
        0,
        true,
        vec![MinuteAssignmentSegment {
            start_unix,
            end_unix: utc_day_start_unix,
            rx_bytes: 3,
            tx_bytes: 2,
        }],
    );

    let rollups = prepare_import_rollups(&prepared, utc_day_start_unix, raw_cutoff_unix).unwrap();
    let old_minutes = (raw_cutoff_unix - start_unix) / 60;
    assert_eq!(
        rollups.iter().map(|rollup| rollup.rx_bytes).sum::<u64>(),
        old_minutes * 3
    );
    assert_eq!(
        rollups.iter().map(|rollup| rollup.tx_bytes).sum::<u64>(),
        old_minutes * 2
    );
    assert_eq!(
        rollups
            .iter()
            .map(|rollup| u64::from(rollup.any_valid_count))
            .sum::<u64>(),
        old_minutes
    );
    let tier_366_start = utc_day_start_unix - 366 * 86_400;
    let tier_181_start = utc_day_start_unix - 181 * 86_400;
    let tier_91_start = utc_day_start_unix - 91 * 86_400;
    let expected_rollup_rows = 1 // the exact predecessor bucket
        + (tier_366_start - start_unix) / 86_400
        + (tier_181_start - tier_366_start) / 21_600
        + (tier_91_start - tier_181_start) / 10_800
        + (raw_cutoff_unix - tier_91_start) / 3_600;
    assert_eq!(
        rollups.len(),
        usize::try_from(expected_rollup_rows).unwrap(),
        "the five-year shape must equal the rows implied by the four negotiated traffic tiers"
    );
    for tier in [3_600, 10_800, 21_600, 86_400] {
        assert!(rollups.iter().any(|rollup| rollup.bucket_secs == tier));
    }

    let mut retained = prepared
        .samples_from("agent-a", raw_cutoff_unix - 60)
        .unwrap();
    let first = retained.next().unwrap().unwrap();
    let mut last = first.clone();
    let mut retained_count = 1_u64;
    for sample in retained {
        last = sample.unwrap();
        retained_count += 1;
    }
    assert_eq!(
        first.observed_unix,
        i64::try_from(raw_cutoff_unix - 60).unwrap()
    );
    assert_eq!(
        last.observed_unix,
        i64::try_from(utc_day_start_unix - 60).unwrap()
    );
    assert_eq!(
        retained_count,
        u64::try_from(TRAFFIC_COUNTER_RAW_RETENTION_DAYS).unwrap() * 24 * 60 + 1
    );
    assert_eq!(
        (first.rx_bytes, first.tx_bytes),
        (
            i64::try_from((raw_cutoff_unix - start_unix) / 60 * 3).unwrap(),
            i64::try_from((raw_cutoff_unix - start_unix) / 60 * 2).unwrap(),
        )
    );
    assert_eq!(
        (last.rx_bytes, last.tx_bytes),
        (
            i64::try_from((utc_day_start_unix - start_unix) / 60 * 3).unwrap(),
            i64::try_from((utc_day_start_unix - start_unix) / 60 * 2).unwrap(),
        )
    );
}

#[test]
fn direct_rollups_match_expanded_oracle_at_all_boundaries() {
    let utc_day_start_unix = 1_772_323_200; // 2026-03-01T00:00:00Z
    let raw_cutoff_unix = utc_day_start_unix - TRAFFIC_COUNTER_RAW_RETENTION_DAYS as u64 * 86_400;
    let tier_boundaries = [
        utc_day_start_unix - 366 * 86_400,
        utc_day_start_unix - 181 * 86_400,
        utc_day_start_unix - 91 * 86_400,
        raw_cutoff_unix,
    ];

    for (index, boundary) in tier_boundaries.into_iter().enumerate() {
        let start_unix = boundary - 7_080;
        let end_unix = boundary + 7_320;
        let middle = boundary + 120;
        let include_baseline = index % 2 == 0;
        let prepared = prepared_import_for_test(
            start_unix,
            end_unix,
            700,
            900,
            include_baseline,
            vec![
                MinuteAssignmentSegment {
                    start_unix,
                    end_unix: middle,
                    rx_bytes: 3,
                    tx_bytes: 1,
                },
                MinuteAssignmentSegment {
                    start_unix: middle,
                    end_unix,
                    rx_bytes: 2,
                    tx_bytes: 5,
                },
            ],
        );

        assert_eq!(
            prepare_import_rollups(&prepared, utc_day_start_unix, raw_cutoff_unix).unwrap(),
            expanded_minute_rollup_oracle(&prepared, utc_day_start_unix, raw_cutoff_unix,),
            "direct aggregation diverged at boundary {boundary}",
        );
    }
}

#[test]
fn compact_raw_preparation_matches_sample_iterator_for_every_slice_path() {
    let start_unix = 1_772_323_200 - 20 * 86_400;
    // Exercise the exact largest accepted raw shape: one complete day,
    // one partial boundary hour, and the optional sequencing predecessor.
    let raw_days = TRAFFIC_COUNTER_RAW_RETENTION_DAYS as u64;
    let end_unix = start_unix + raw_days * 86_400 + 3_600;
    let middle = start_unix + 12 * 3_600;
    let raw_cutoff_unix = end_unix - raw_days * 86_400;
    for include_baseline in [false, true] {
        let prepared = prepared_import_for_test(
            start_unix,
            end_unix,
            700,
            900,
            include_baseline,
            vec![
                MinuteAssignmentSegment {
                    start_unix,
                    end_unix: middle,
                    rx_bytes: 3,
                    tx_bytes: 1,
                },
                MinuteAssignmentSegment {
                    start_unix: middle,
                    end_unix,
                    rx_bytes: 2,
                    tx_bytes: 5,
                },
            ],
        );
        let natural_start = if include_baseline {
            start_unix - 60
        } else {
            start_unix
        };
        for minimum_unix in [
            natural_start,
            start_unix,
            raw_cutoff_unix - 60,
            raw_cutoff_unix,
            end_unix,
        ] {
            let expected = prepared
                .samples_from("agent-a", minimum_unix)
                .unwrap()
                .map(|sample| {
                    let sample = sample.unwrap();
                    (
                        sample.observed_unix,
                        sample.rx_bytes,
                        sample.tx_bytes,
                        sample.observed_unix < i64::try_from(raw_cutoff_unix).unwrap(),
                    )
                })
                .collect::<Vec<_>>();
            let actual = prepare_import_raw_rows(&prepared, minimum_unix, raw_cutoff_unix).unwrap();
            assert_eq!(
                expected,
                actual
                    .observed_unix
                    .iter()
                    .copied()
                    .zip(actual.rx_bytes.iter().copied())
                    .zip(actual.tx_bytes.iter().copied())
                    .zip(actual.inbound_promoted.iter().copied())
                    .map(
                        |(((observed_unix, rx_bytes), tx_bytes), inbound_promoted)| {
                            (observed_unix, rx_bytes, tx_bytes, inbound_promoted)
                        }
                    )
                    .collect::<Vec<_>>(),
                "compact raw preparation diverged at {minimum_unix} (baseline={include_baseline})",
            );
        }

        let superset_minimum =
            postgres_import_raw_superset_minimum(&prepared, raw_cutoff_unix).unwrap();
        let superset =
            prepare_import_raw_rows(&prepared, superset_minimum, raw_cutoff_unix).unwrap();
        for plan_minimum in [superset_minimum, raw_cutoff_unix, end_unix] {
            let expected = prepared
                .samples_from("agent-a", plan_minimum)
                .unwrap()
                .map(|sample| {
                    let sample = sample.unwrap();
                    (sample.observed_unix, sample.rx_bytes, sample.tx_bytes)
                })
                .collect::<Vec<_>>();
            let plan_minimum = i64::try_from(plan_minimum).unwrap();
            let start_index = superset
                .observed_unix
                .partition_point(|observed_unix| *observed_unix < plan_minimum);
            assert_eq!(
                expected,
                superset.observed_unix[start_index..]
                    .iter()
                    .copied()
                    .zip(superset.rx_bytes[start_index..].iter().copied())
                    .zip(superset.tx_bytes[start_index..].iter().copied())
                    .map(|((observed_unix, rx_bytes), tx_bytes)| {
                        (observed_unix, rx_bytes, tx_bytes)
                    })
                    .collect::<Vec<_>>()
            );
        }
    }
}

#[test]
fn same_shape_raw_update_requires_an_exact_dense_locked_keyset() {
    let prepared = prepared_import_for_test(
        1_772_323_200,
        1_772_323_200 + 6 * 60,
        0,
        0,
        true,
        vec![MinuteAssignmentSegment {
            start_unix: 1_772_323_200,
            end_unix: 1_772_323_200 + 6 * 60,
            rx_bytes: 3,
            tx_bytes: 2,
        }],
    );
    let raw = prepare_import_raw_rows(&prepared, 1_772_323_140, 1_772_323_200 + 60).unwrap();
    let plan = PostgresImportRawPlan {
        minimum_unix: 1_772_323_140,
        rx_counter_epoch: 0,
        tx_counter_epoch: 0,
        delete_inbound_predecessor_unix: None,
        successor_adjustment: None,
    };
    let exact = PostgresImportOwnedRawStats {
        interface: "eth0".to_string(),
        count: i64::try_from(raw.observed_unix.len()).unwrap(),
        first_observed_unix: raw.observed_unix.first().copied(),
        last_observed_unix: raw.observed_unix.last().copied(),
    };
    assert!(postgres_import_can_update_same_shape(&exact, &raw, &plan).unwrap());

    let mut shifted = exact.clone();
    shifted.first_observed_unix = shifted.first_observed_unix.map(|value| value + 60);
    assert!(!postgres_import_can_update_same_shape(&shifted, &raw, &plan).unwrap());

    let mut missing = exact.clone();
    missing.count -= 1;
    assert!(!postgres_import_can_update_same_shape(&missing, &raw, &plan).unwrap());

    let mut sparse = raw.clone();
    sparse.observed_unix[2] += 60;
    assert!(postgres_import_can_update_same_shape(&exact, &sparse, &plan).is_err());

    let mut empty_plan = plan;
    empty_plan.minimum_unix = 1_772_323_200 + 6 * 60;
    let empty_raw =
        prepare_import_raw_rows(&prepared, empty_plan.minimum_unix, 1_772_323_200 + 6 * 60)
            .unwrap();
    let empty_stats = PostgresImportOwnedRawStats {
        interface: "eth0".to_string(),
        count: 0,
        first_observed_unix: None,
        last_observed_unix: None,
    };
    assert!(postgres_import_can_update_same_shape(&empty_stats, &empty_raw, &empty_plan).unwrap());
    assert!(!postgres_import_can_update_same_shape(&exact, &empty_raw, &empty_plan).unwrap());
}

#[test]
fn compact_rollup_arrays_preserve_direct_rollup_values() {
    let utc_day_start_unix = 1_772_323_200;
    let raw_cutoff_unix = utc_day_start_unix - TRAFFIC_COUNTER_RAW_RETENTION_DAYS as u64 * 86_400;
    let start_unix = utc_day_start_unix - 400 * 86_400;
    let prepared = prepared_import_for_test(
        start_unix,
        utc_day_start_unix,
        0,
        0,
        true,
        vec![MinuteAssignmentSegment {
            start_unix,
            end_unix: utc_day_start_unix,
            rx_bytes: 3,
            tx_bytes: 2,
        }],
    );
    let rollups = prepare_import_rollups(&prepared, utc_day_start_unix, raw_cutoff_unix).unwrap();
    let rows = prepare_import_rollup_rows(&prepared.interface, rollups.clone()).unwrap();

    assert_eq!(
        rows.bucket_secs,
        rollups
            .iter()
            .map(|rollup| rollup.bucket_secs)
            .collect::<Vec<_>>()
    );
    assert_eq!(
        rows.bucket_start_unix,
        rollups
            .iter()
            .map(|rollup| i64::try_from(rollup.bucket_start_unix).unwrap())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        rows.rx_bytes,
        rollups
            .iter()
            .map(|rollup| i64::try_from(rollup.rx_bytes).unwrap())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        rows.latest_observed_unix,
        rollups
            .iter()
            .map(|rollup| i64::try_from(rollup.latest_observed_unix).unwrap())
            .collect::<Vec<_>>()
    );
}

#[test]
fn postgres_preflight_revalidation_detects_every_mutable_descriptor_group() {
    let boundary = sample_record(
        "agent-a",
        "eth0",
        1_772_323_200,
        100,
        200,
        "interface_counters",
    )
    .unwrap();
    let snapshot = PostgresImportSnapshot {
        utc_day_start_unix: 1_772_323_200,
        raw_cutoff_unix: 1_772_323_200 - TRAFFIC_COUNTER_RAW_RETENTION_DAYS as u64 * 86_400,
        boundary_samples: vec![boundary],
        imported_raw_stats: vec![PostgresImportOwnedRawStats {
            interface: "eth0".to_string(),
            count: 1,
            first_observed_unix: Some(1_772_323_140),
            last_observed_unix: Some(1_772_323_140),
        }],
    };
    assert!(postgres_import_snapshots_match(
        &snapshot,
        &snapshot.clone()
    ));

    let mut changed_retention = snapshot.clone();
    changed_retention.raw_cutoff_unix += 86_400;
    assert!(!postgres_import_snapshots_match(
        &snapshot,
        &changed_retention
    ));
    let mut changed_boundary = snapshot.clone();
    changed_boundary.boundary_samples[0].rx_bytes += 1;
    assert!(!postgres_import_snapshots_match(
        &snapshot,
        &changed_boundary
    ));
    let mut changed_owned_rows = snapshot.clone();
    changed_owned_rows.imported_raw_stats[0].count += 1;
    assert!(!postgres_import_snapshots_match(
        &snapshot,
        &changed_owned_rows
    ));

    // The repeatable-read preflight deliberately carries no owned-row stats;
    // the locked snapshot remains the authoritative bound/shape check.
    let mut shape_only_preflight = snapshot.clone();
    shape_only_preflight.imported_raw_stats.clear();
    assert!(postgres_import_snapshots_match(
        &shape_only_preflight,
        &changed_owned_rows
    ));
}

#[test]
fn imported_raw_recovery_guard_accepts_exact_canonical_maximum_only() {
    assert_eq!(
        POSTGRES_IMPORT_MAX_RAW_ROWS_PER_INTERFACE,
        (usize::try_from(TRAFFIC_COUNTER_RAW_RETENTION_DAYS).unwrap() * 24 + 1) * 60 + 1
    );
    assert_eq!(POSTGRES_IMPORT_MAX_RAW_ROWS_PER_INTERFACE, 1_501);
    let utc_day_start_unix = 1_772_323_200;
    let current_hour_unix = utc_day_start_unix + 23 * 3_600;
    let raw_cutoff_unix = current_hour_unix - TRAFFIC_COUNTER_RAW_RETENTION_DAYS as u64 * 86_400;
    let last_observed_unix = current_hour_unix + 59 * 60;
    let mut snapshot = PostgresImportSnapshot {
        utc_day_start_unix,
        raw_cutoff_unix,
        boundary_samples: Vec::new(),
        imported_raw_stats: vec![PostgresImportOwnedRawStats {
            interface: "eth0".to_string(),
            count: i64::try_from(POSTGRES_IMPORT_MAX_RAW_ROWS_PER_INTERFACE).unwrap(),
            first_observed_unix: Some(i64::try_from(raw_cutoff_unix - 60).unwrap()),
            last_observed_unix: Some(i64::try_from(last_observed_unix).unwrap()),
        }],
    };
    ensure_postgres_import_owned_raw_is_bounded(&snapshot).unwrap();

    snapshot.imported_raw_stats[0].count += 1;
    let error = ensure_postgres_import_owned_raw_is_bounded(&snapshot)
        .unwrap_err()
        .to_string();
    assert!(error.contains("network_traffic_import_recovery_required"));
    assert!(error.contains("max_1501"));
}

#[test]
fn five_year_raw_superset_is_bounded_at_maximum_for_all_sixteen_interfaces() {
    assert_eq!(NETWORK_TRAFFIC_IMPORT_MAX_INTERFACES, 16);
    let utc_day_start_unix = 1_772_323_200;
    let current_hour_unix = utc_day_start_unix + 23 * 3_600;
    let raw_cutoff_unix = current_hour_unix - TRAFFIC_COUNTER_RAW_RETENTION_DAYS as u64 * 86_400;
    let start_unix = utc_day_start_unix - 5 * 365 * 86_400;
    // Import coverage is end-exclusive. Ending at the next hour includes the
    // current hour's :59 sample, so this exercises the exact worst case: the
    // one-day floor-aligned window, one partial hour, and one predecessor.
    let end_unix = current_hour_unix + 60 * 60;
    let base = prepared_import_for_test(
        start_unix,
        end_unix,
        0,
        0,
        true,
        vec![MinuteAssignmentSegment {
            start_unix,
            end_unix,
            rx_bytes: 3,
            tx_bytes: 2,
        }],
    );
    let mut total_rows = 0_usize;
    for index in 0..NETWORK_TRAFFIC_IMPORT_MAX_INTERFACES {
        let mut prepared = base.clone();
        prepared.interface = format!("eth{index}");
        let minimum = postgres_import_raw_superset_minimum(&prepared, raw_cutoff_unix).unwrap();
        assert_eq!(minimum, raw_cutoff_unix - 60);
        let rows = prepare_import_raw_rows(&prepared, minimum, raw_cutoff_unix).unwrap();
        assert_eq!(
            rows.observed_unix.len(),
            POSTGRES_IMPORT_MAX_RAW_ROWS_PER_INTERFACE
        );
        total_rows += rows.observed_unix.len();
    }
    assert_eq!(
        total_rows,
        NETWORK_TRAFFIC_IMPORT_MAX_INTERFACES * POSTGRES_IMPORT_MAX_RAW_ROWS_PER_INTERFACE
    );
}
