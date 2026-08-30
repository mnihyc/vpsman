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

fn fleet_telemetry_invalidation_counts(
    browser_clients: &mut [broadcast::Receiver<WsEvent>],
) -> Vec<usize> {
    browser_clients
        .iter_mut()
        .map(|client| fleet_telemetry_invalidation_count(&drain_events(client)))
        .collect()
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
async fn fleet_telemetry_frames_coalesce_once_per_browser_per_delayed_boundary() {
    assert_eq!(FLEET_TELEMETRY_INVALIDATION_WINDOW, Duration::from_secs(2));
    let (events, invalidations) = WsEventBus::new(512);
    let mut browser_clients = (0..5).map(|_| events.subscribe()).collect::<Vec<_>>();
    let coalescer = spawn_ws_invalidation_coalescer(events.clone(), invalidations);
    tokio::task::yield_now().await;

    // Six accepted frames from each of 120 VPSs still set one fleet-wide
    // pending flag. Every connected browser receives one refetch notice at
    // the boundary, rather than one notice per VPS or per frame.
    for _accepted_frame in 0..6 {
        for _vps in 0..120 {
            events.notify_fleet_telemetry();
        }
    }
    assert_eq!(
        fleet_telemetry_invalidation_counts(&mut browser_clients),
        vec![0; 5]
    );

    time::advance(Duration::from_millis(1_999)).await;
    tokio::task::yield_now().await;
    assert_eq!(
        fleet_telemetry_invalidation_counts(&mut browser_clients),
        vec![0; 5]
    );

    time::advance(Duration::from_millis(1)).await;
    tokio::task::yield_now().await;
    assert_eq!(
        fleet_telemetry_invalidation_counts(&mut browser_clients),
        vec![1; 5]
    );

    time::advance(Duration::from_secs(1)).await;
    for _ in 0..120 {
        events.notify_fleet_telemetry();
    }
    time::advance(Duration::from_millis(999)).await;
    events.notify_fleet_telemetry();
    assert_eq!(
        fleet_telemetry_invalidation_counts(&mut browser_clients),
        vec![0; 5]
    );

    time::advance(Duration::from_millis(1)).await;
    tokio::task::yield_now().await;
    assert_eq!(
        fleet_telemetry_invalidation_counts(&mut browser_clients),
        vec![1; 5]
    );

    time::advance(FLEET_TELEMETRY_INVALIDATION_WINDOW).await;
    tokio::task::yield_now().await;
    assert_eq!(
        fleet_telemetry_invalidation_counts(&mut browser_clients),
        vec![0; 5]
    );

    // If the coalescer misses several boundaries, Delay emits only the one
    // pending notice. A later notice then waits a complete fresh boundary;
    // missed ticks never replay as a burst to browser clients.
    events.notify_fleet_telemetry();
    time::advance(Duration::from_secs(10)).await;
    tokio::task::yield_now().await;
    assert_eq!(
        fleet_telemetry_invalidation_counts(&mut browser_clients),
        vec![1; 5]
    );

    events.notify_fleet_telemetry();
    tokio::task::yield_now().await;
    assert_eq!(
        fleet_telemetry_invalidation_counts(&mut browser_clients),
        vec![0; 5]
    );
    time::advance(Duration::from_millis(1_999)).await;
    tokio::task::yield_now().await;
    assert_eq!(
        fleet_telemetry_invalidation_counts(&mut browser_clients),
        vec![0; 5]
    );
    time::advance(Duration::from_millis(1)).await;
    tokio::task::yield_now().await;
    assert_eq!(
        fleet_telemetry_invalidation_counts(&mut browser_clients),
        vec![1; 5]
    );

    coalescer.abort();
    assert!(coalescer.await.unwrap_err().is_cancelled());
}

#[tokio::test(start_paused = true)]
async fn fleet_telemetry_boundary_invalidates_cache_before_refetch_event() {
    let (events, invalidations) = WsEventBus::new(16);
    let mut observed = events.subscribe();
    let coalescer = spawn_ws_invalidation_coalescer(events.clone(), invalidations);
    tokio::task::yield_now().await;

    events.notify_fleet_telemetry();
    time::advance(Duration::from_millis(1_500)).await;
    let computations = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let first_computations = std::sync::Arc::clone(&computations);
    let first = events
        .singleflight_monitoring_cards("boundary".to_string(), move || async move {
            let total = first_computations.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            Ok(crate::model::MonitoringCardsPageView {
                items: Vec::new(),
                offset: 0,
                limit: 1,
                total,
                next_offset: None,
            })
        })
        .await
        .unwrap();
    assert_eq!(first.total, 1);

    // The cached result is only 500 ms old when the fixed boundary fires.
    time::advance(Duration::from_millis(500)).await;
    tokio::task::yield_now().await;
    assert_eq!(
        fleet_telemetry_invalidation_count(&drain_events(&mut observed)),
        1
    );

    let refreshed_computations = std::sync::Arc::clone(&computations);
    let refreshed = events
        .singleflight_monitoring_cards("boundary".to_string(), move || async move {
            let total =
                refreshed_computations.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            Ok(crate::model::MonitoringCardsPageView {
                items: Vec::new(),
                offset: 0,
                limit: 1,
                total,
                next_offset: None,
            })
        })
        .await
        .unwrap();
    assert_eq!(refreshed.total, 2);
    assert_eq!(computations.load(std::sync::atomic::Ordering::SeqCst), 2);

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
