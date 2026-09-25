use super::*;
use std::os::unix::fs::PermissionsExt;

#[test]
fn network_observation_compact_format_is_opt_in_and_keeps_query_filters() {
    let uri = "/api/v1/network/observations?window=custom&start_unix=100&end_unix=200&limit=250000&source=manual&client_id=endpoint-a&format=compact"
        .parse()
        .unwrap();
    let query = Query::<NetworkEvidenceQuery>::try_from_uri(&uri).unwrap().0;
    let format = Query::<NetworkObservationFormatQuery>::try_from_uri(&uri)
        .unwrap()
        .0;
    assert!(matches!(
        format.format,
        Some(NetworkObservationFormat::Compact)
    ));
    let filter = network_observation_filter(&query, 100_000, true).unwrap();
    assert_eq!(
        (filter.start_unix, filter.end_unix, filter.limit),
        (100, 200, 250_000)
    );
    assert_eq!(filter.source.as_deref(), Some("manual"));
    assert_eq!(filter.client_id.as_deref(), Some("endpoint-a"));
    assert!(filter.visible_only);
    for path in [
        "/api/v1/network/observations",
        "/api/v1/network/observations?limit=1",
    ] {
        let query = Query::<NetworkObservationFormatQuery>::try_from_uri(&path.parse().unwrap())
            .unwrap()
            .0;
        assert!(query.format.is_none());
        assert_eq!(
            serde_json::to_value(NetworkObservationsResponse::new(vec![], query.format)).unwrap(),
            serde_json::json!([])
        );
    }
    assert!(Query::<NetworkObservationFormatQuery>::try_from_uri(
        &"/api/v1/network/observations?format=unsupported"
            .parse()
            .unwrap()
    )
    .is_err());
    let empty = serde_json::to_value(NetworkObservationsResponse::new(
        vec![],
        Some(NetworkObservationFormat::Compact),
    ))
    .unwrap();
    assert_eq!(
        empty["fields"],
        serde_json::json!(NETWORK_OBSERVATION_FIELDS)
    );
    assert_eq!(empty["rows"], serde_json::json!([]));
}

#[test]
fn network_observation_compact_rows_losslessly_preserve_all_fields() {
    let manual = NetworkObservationView {
        id: Uuid::from_u128(3),
        job_id: Some(Uuid::from_u128(2)),
        client_id: "endpoint-a".into(),
        seq: Some(0),
        kind: "network_speed_test".into(),
        source: "manual".into(),
        role: Some("client".into()),
        plan_id: Some(Uuid::from_u128(1)),
        topology_identity_hash: Some("a".repeat(64)),
        plan_name: Some("Plan \"quoted\" / 隧道".into()),
        interface_name: Some("".into()),
        peer_client_id: Some("endpoint-b".into()),
        target: Some("[2001:db8::1]:5201".into()),
        endpoint_side: Some("right".into()),
        address_family: Some("ipv6".into()),
        stale_after_secs: Some(180),
        healthy: Some(false),
        transmitted: Some(5),
        received: Some(0),
        latency_min_ms: Some(0.0),
        latency_avg_ms: Some(1.25),
        latency_max_ms: Some(3.5),
        latency_mdev_ms: Some(0.75),
        packet_loss_ratio: Some(1.0),
        reason: Some("line\nquote\"".into()),
        throughput_mbps: Some(42.125),
        bytes: Some(9_007_199_254_740_991),
        metadata: serde_json::json!({"": null, "nested": [true, false, 0, -1.25, {"id": "not-a-column", "__proto__": {"safe": true}}]}),
        observed_at: "2026-09-26T01:02:03.123456Z".into(),
        received_at: "2026-09-26T01:02:04Z".into(),
    };
    let automatic = NetworkObservationView {
        id: Uuid::from_u128(4),
        job_id: None,
        client_id: "".into(),
        seq: None,
        kind: "tunnel_reachability".into(),
        source: "automatic".into(),
        role: None,
        plan_id: None,
        topology_identity_hash: None,
        plan_name: None,
        interface_name: None,
        peer_client_id: None,
        target: None,
        endpoint_side: None,
        address_family: None,
        stale_after_secs: None,
        healthy: None,
        transmitted: None,
        received: None,
        latency_min_ms: None,
        latency_avg_ms: None,
        latency_max_ms: None,
        latency_mdev_ms: None,
        packet_loss_ratio: None,
        reason: None,
        throughput_mbps: None,
        bytes: None,
        metadata: serde_json::Value::Null,
        observed_at: "2026-09-26T01:02:00Z".into(),
        received_at: "".into(),
    };
    let mut observations = vec![manual, automatic.clone()];
    // Metadata is arbitrary JSON, not only an object. Keep every value verbatim.
    for metadata in [
        serde_json::json!([]),
        serde_json::json!({}),
        serde_json::json!(""),
        serde_json::json!(false),
        serde_json::json!(12.5),
    ] {
        observations.push(NetworkObservationView {
            metadata,
            ..automatic.clone()
        });
    }
    let expected = serde_json::to_value(&observations).unwrap();
    assert_eq!(
        serde_json::to_vec(&NetworkObservationsResponse::new(
            observations.clone(),
            None
        ))
        .unwrap(),
        serde_json::to_vec(&observations).unwrap()
    );
    let compact = serde_json::to_value(NetworkObservationsResponse::new(
        observations,
        Some(NetworkObservationFormat::Compact),
    ))
    .unwrap();
    let fields = compact["fields"].as_array().unwrap();
    assert_eq!(fields.len(), expected[0].as_object().unwrap().len());
    assert_eq!(
        fields
            .iter()
            .map(|field| field.as_str().unwrap())
            .collect::<BTreeSet<_>>()
            .len(),
        fields.len()
    );
    let reconstructed = compact["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            let values = row.as_array().unwrap();
            assert_eq!(values.len(), fields.len());
            serde_json::Value::Object(
                fields
                    .iter()
                    .zip(values)
                    .map(|(field, value)| (field.as_str().unwrap().to_string(), value.clone()))
                    .collect(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(serde_json::json!(reconstructed), expected);
    assert!(
        serde_json::to_vec(&compact).unwrap().len() < serde_json::to_vec(&expected).unwrap().len()
    );
}

#[test]
fn temp_download_file_uses_private_spool_directory() {
    let temp = TempDownloadFile::new("vpsman-test-download", "bin").unwrap();
    let parent = temp.path().parent().unwrap();

    assert_eq!(
        std::fs::metadata(parent).unwrap().permissions().mode() & 0o777,
        0o700
    );
}

#[test]
fn process_inventory_scan_limit_is_an_explicit_incomplete_signal() {
    let error = map_process_supervisor_inventory_error(anyhow::anyhow!(
        PROCESS_SUPERVISOR_INVENTORY_SCAN_LIMIT_ERROR
    ));

    assert_eq!(error.status, axum::http::StatusCode::CONFLICT);
    assert_eq!(error.code, PROCESS_SUPERVISOR_INVENTORY_SCAN_LIMIT_ERROR);
    assert!(error
        .public_message
        .as_deref()
        .is_some_and(|message| message.contains("Exact process inventory")));
}

#[test]
fn exact_job_target_status_batch_validates_pair_identity_and_bound() {
    let first_job = Uuid::new_v4();
    let second_job = Uuid::new_v4();
    assert!(validate_job_target_status_batch(&[
        JobTargetStatusBatchItem {
            job_id: first_job,
            client_id: "v-1".to_string(),
        },
        JobTargetStatusBatchItem {
            job_id: first_job,
            client_id: "v-2".to_string(),
        },
        JobTargetStatusBatchItem {
            job_id: second_job,
            client_id: "v-1".to_string(),
        },
    ])
    .is_ok());
    assert_eq!(
        validate_job_target_status_batch(&[])
            .expect_err("empty exact-pair request must fail")
            .code,
        "job_target_status_pairs_invalid"
    );
    assert_eq!(
        validate_job_target_status_batch(&[
            JobTargetStatusBatchItem {
                job_id: first_job,
                client_id: "v-1".to_string(),
            },
            JobTargetStatusBatchItem {
                job_id: first_job,
                client_id: "v-1".to_string(),
            },
        ])
        .expect_err("duplicate exact pair must fail")
        .code,
        "job_target_status_pairs_duplicate"
    );
    let oversized = (0..=JOB_TARGET_STATUS_BATCH_MAX_ITEMS)
        .map(|index| JobTargetStatusBatchItem {
            job_id: Uuid::new_v4(),
            client_id: format!("v-{index}"),
        })
        .collect::<Vec<_>>();
    assert_eq!(
        validate_job_target_status_batch(&oversized)
            .expect_err("oversized exact-pair request must fail")
            .code,
        "job_target_status_pairs_invalid"
    );
}
