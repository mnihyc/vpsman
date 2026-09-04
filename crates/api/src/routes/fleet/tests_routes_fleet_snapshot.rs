use super::*;

#[tokio::test]
async fn bounded_snapshot_sources_distinguish_exact_boundary_from_overflow() {
    let (exact, exact_truncated) =
        load_bounded_source("test", true, async { Ok::<_, anyhow::Error>(vec![0; 200]) }).await;
    assert_eq!(exact.data.unwrap().len(), 200);
    assert!(!exact_truncated);

    let (overflow, overflow_truncated) =
        load_bounded_source("test", true, async { Ok::<_, anyhow::Error>(vec![0; 201]) }).await;
    assert_eq!(overflow.data.unwrap().len(), 200);
    assert!(overflow_truncated);
}

#[tokio::test]
async fn snapshot_source_failures_name_the_failed_source_without_leaking_the_cause() {
    let source: FleetSnapshotSource<Vec<String>> =
        load_source("telemetry_network_rates", true, async {
            Err(anyhow::anyhow!("private database address and query"))
        })
        .await;

    assert!(source.data.is_none());
    assert_eq!(
        source.error.as_deref(),
        Some("fleet_snapshot_telemetry_network_rates_unavailable")
    );
    assert!(!source.error.unwrap().contains("database"));
}
