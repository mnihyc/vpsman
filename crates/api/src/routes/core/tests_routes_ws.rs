use super::*;

fn drain_events(events: &mut broadcast::Receiver<WsEvent>) -> Vec<WsEvent> {
    let mut drained = Vec::new();
    loop {
        match events.try_recv() {
            Ok(event) => drained.push(event),
            Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
            Err(broadcast::error::TryRecvError::Empty | broadcast::error::TryRecvError::Closed) => {
                return drained;
            }
        }
    }
}

fn fleet_telemetry_invalidation_count(events: &[WsEvent]) -> usize {
    events
        .iter()
        .filter(|event| matches!(event, WsEvent::FleetTelemetryInvalidated))
        .count()
}

fn job_detail_invalidations(events: &[WsEvent]) -> Vec<Vec<uuid::Uuid>> {
    events
        .iter()
        .filter_map(|event| match event {
            WsEvent::JobDetailsInvalidated { job_ids } => Some(job_ids.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn fleet_telemetry_invalidation_is_an_exact_unit_wire_event() {
    assert_eq!(
        serde_json::to_value(WsEvent::FleetTelemetryInvalidated).unwrap(),
        serde_json::json!({"type": "fleet_telemetry_invalidated"})
    );
}

#[test]
fn job_detail_invalidation_has_only_sorted_job_ids_on_the_wire() {
    let first = uuid::Uuid::from_u128(1);
    let second = uuid::Uuid::from_u128(2);
    assert_eq!(
        serde_json::to_value(WsEvent::JobDetailsInvalidated {
            job_ids: vec![first, second],
        })
        .unwrap(),
        serde_json::json!({
            "type": "job_details_invalidated",
            "job_ids": [first, second],
        })
    );
}

#[tokio::test(start_paused = true)]
async fn fleet_telemetry_updates_share_one_fixed_non_sliding_window() {
    let (events, invalidations) = WsEventBus::new(512);
    let mut observed = events.subscribe();
    let coalescer = spawn_ws_invalidation_coalescer(events.clone(), invalidations);
    tokio::task::yield_now().await;

    for _ in 0..200 {
        events.invalidate_fleet_telemetry();
    }
    assert!(drain_events(&mut observed).is_empty());

    time::advance(Duration::from_millis(14_999)).await;
    tokio::task::yield_now().await;
    assert_eq!(
        fleet_telemetry_invalidation_count(&drain_events(&mut observed)),
        0
    );

    time::advance(Duration::from_millis(1)).await;
    tokio::task::yield_now().await;
    assert_eq!(
        fleet_telemetry_invalidation_count(&drain_events(&mut observed)),
        1
    );

    time::advance(Duration::from_secs(10)).await;
    for _ in 0..100 {
        events.invalidate_fleet_telemetry();
    }
    time::advance(Duration::from_millis(4_999)).await;
    events.invalidate_fleet_telemetry();
    assert_eq!(
        fleet_telemetry_invalidation_count(&drain_events(&mut observed)),
        0
    );

    time::advance(Duration::from_millis(1)).await;
    tokio::task::yield_now().await;
    assert_eq!(
        fleet_telemetry_invalidation_count(&drain_events(&mut observed)),
        1
    );

    time::advance(FLEET_TELEMETRY_INVALIDATION_WINDOW).await;
    tokio::task::yield_now().await;
    assert_eq!(
        fleet_telemetry_invalidation_count(&drain_events(&mut observed)),
        0
    );

    coalescer.abort();
    assert!(coalescer.await.unwrap_err().is_cancelled());
}

#[tokio::test(start_paused = true)]
async fn job_output_notices_share_one_sorted_fixed_non_sliding_window() {
    let first = uuid::Uuid::from_u128(1);
    let second = uuid::Uuid::from_u128(2);
    let third = uuid::Uuid::from_u128(3);
    let (events, invalidations) = WsEventBus::new(32);
    let mut observed = events.subscribe();
    let coalescer = spawn_ws_invalidation_coalescer(events.clone(), invalidations);
    tokio::task::yield_now().await;

    for job_id in [second, first, second] {
        events.invalidate_job_details(job_id);
    }
    assert!(drain_events(&mut observed).is_empty());
    time::advance(Duration::from_millis(4_999)).await;
    events.invalidate_job_details(third);
    assert!(job_detail_invalidations(&drain_events(&mut observed)).is_empty());

    time::advance(Duration::from_millis(1)).await;
    tokio::task::yield_now().await;
    assert_eq!(
        job_detail_invalidations(&drain_events(&mut observed)),
        vec![vec![first, second, third]]
    );

    time::advance(Duration::from_secs(4)).await;
    events.invalidate_job_details(second);
    time::advance(Duration::from_millis(999)).await;
    events.invalidate_job_details(first);
    assert!(job_detail_invalidations(&drain_events(&mut observed)).is_empty());

    time::advance(Duration::from_millis(1)).await;
    tokio::task::yield_now().await;
    assert_eq!(
        job_detail_invalidations(&drain_events(&mut observed)),
        vec![vec![first, second]]
    );

    time::advance(JOB_DETAILS_INVALIDATION_WINDOW).await;
    tokio::task::yield_now().await;
    assert!(job_detail_invalidations(&drain_events(&mut observed)).is_empty());

    coalescer.abort();
    assert!(coalescer.await.unwrap_err().is_cancelled());
}
