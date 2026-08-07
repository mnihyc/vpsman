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

#[test]
fn finer_rows_are_preserved_and_only_coarse_residual_is_distributed() {
    let start = 1_722_470_400;
    let end = start + 3_600;
    let buckets = vec![
        bucket(start, 3_600, 3_600, 1_800),
        bucket(start, 300, 1_000, 500),
    ];

    let (minutes, rx, tx) =
        expand_buckets_to_minutes(&buckets, "eth0", start, end).unwrap();

    assert_eq!(minutes.len(), 60);
    assert_eq!(rx, 3_600);
    assert_eq!(tx, 1_800);
    assert_eq!(minutes.iter().take(5).map(|row| row.1).sum::<u64>(), 1_000);
    assert_eq!(minutes.iter().take(5).map(|row| row.2).sum::<u64>(), 500);
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
    let existing = vec![sample_record(
        "agent-a",
        "eth0",
        live,
        10,
        20,
        "interface_counters",
    )
    .unwrap()];
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

    assert_eq!(prepared[0].end_unix, live);
    assert_eq!(
        prepared[0].samples.first().unwrap().observed_unix,
        (start - 60) as i64
    );
    assert_eq!(
        prepared[0].samples.last().unwrap().observed_unix,
        (live - 60) as i64
    );
}
