use super::*;
use std::os::unix::fs::PermissionsExt;
use tokio::sync::oneshot;

static TEST_ENQUEUE_SEQ: AtomicU64 = AtomicU64::new(1);

async fn take_replay_items(
    forwarder: &GatewayEventForwarder,
    index: &mut GatewaySpoolReplayIndex,
) -> Vec<(String, GatewayForwardQueueItem)> {
    forwarder.spool.refresh_replay_index(index).await.unwrap();
    let mut items = Vec::new();
    while let Some(candidate) = index.pop_next() {
        let target_key = candidate.target_key.clone();
        items.push((candidate.target_key.clone(), candidate.into_queue_item()));
        index.schedule_if_candidates(&target_key);
    }
    items
}

fn synthetic_spool_candidate(target_key: &str, enqueue_seq: u64) -> GatewayPendingSpoolCandidate {
    GatewayPendingSpoolCandidate {
        target_key: target_key.to_string(),
        path: std::env::temp_dir().join(format!(
            "vpsman-synthetic-replay-{target_key}-{enqueue_seq}.spool"
        )),
        created_unix: enqueue_seq,
        enqueue_seq,
        disk_bytes: 1,
    }
}

fn test_event(path: &str, body: &[u8]) -> GatewayForwardEvent {
    let kind = GatewayForwardEventKind::for_path(path);
    GatewayForwardEvent {
        api_url: "http://127.0.0.1:9".to_string(),
        path: path.to_string(),
        body: body.to_vec(),
        internal_token: Some("test-token".to_string()),
        kind,
        critical: gateway_event_critical(kind, body),
        command_output: None,
        gateway_session_id: None,
        telemetry_route_refresh_owner: None,
        created_at: time::Instant::now(),
        created_unix: unix_now(),
        enqueue_seq: TEST_ENQUEUE_SEQ.fetch_add(1, Ordering::Relaxed),
    }
}

fn terminal_output_event(
    job_id: uuid::Uuid,
    stream: vpsman_common::OutputStream,
    terminal_seq: Option<u64>,
    done: bool,
    data: Vec<u8>,
) -> GatewayTerminalOutputIngest {
    GatewayTerminalOutputIngest {
        gateway_id: "gateway-a".to_string(),
        gateway_session_id: uuid::Uuid::new_v4(),
        process_incarnation_id: uuid::Uuid::new_v4(),
        spooled_replay: false,
        client_id: "client-a".to_string(),
        output: vpsman_common::TerminalStreamOutput {
            job_id,
            session_id: uuid::Uuid::new_v4(),
            terminal_seq,
            output_first_seq: Some(1),
            output_next_seq: terminal_seq.unwrap_or(1).saturating_add(1),
            output_retained_first_seq: Some(1),
            output_retained_bytes: data.len() as u64,
            output_dropped_bytes: 0,
            output_dropped_chunks: 0,
            output_replay_truncated: false,
            output: vpsman_common::CommandOutput {
                job_id,
                stream,
                data,
                exit_code: done.then_some(0),
                done,
            },
        },
    }
}

async fn single_response_server(status: &str, body: &str) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = [0_u8; 4096];
        let _ = socket.read(&mut request).await;
        socket.write_all(response.as_bytes()).await.unwrap();
    });
    format!("http://{addr}")
}

async fn read_http_body(socket: &mut tokio::net::TcpStream) -> Vec<u8> {
    let mut bytes = Vec::new();
    let header_end = loop {
        let mut chunk = [0_u8; 4096];
        let read = socket.read(&mut chunk).await.unwrap();
        assert!(read > 0);
        bytes.extend_from_slice(&chunk[..read]);
        if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break end + 4;
        }
    };
    let headers = String::from_utf8_lossy(&bytes[..header_end]);
    let content_length = headers
        .lines()
        .find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .and_then(|value| value.trim().parse::<usize>().ok())
        })
        .unwrap_or(0);
    while bytes.len() < header_end + content_length {
        let mut chunk = [0_u8; 4096];
        let read = socket.read(&mut chunk).await.unwrap();
        assert!(read > 0);
        bytes.extend_from_slice(&chunk[..read]);
    }
    bytes[header_end..header_end + content_length].to_vec()
}

async fn forward_once(
    event: &GatewayForwardEvent,
    session_rejection_handler: &StdRwLock<Option<GatewaySessionRejectionHandler>>,
) -> GatewayForwardOutcome {
    let telemetry_route_refresh_handler = StdRwLock::new(None);
    forward_once_with_route_refresh(
        event,
        session_rejection_handler,
        &telemetry_route_refresh_handler,
    )
    .await
}

async fn forward_once_with_route_refresh(
    event: &GatewayForwardEvent,
    session_rejection_handler: &StdRwLock<Option<GatewaySessionRejectionHandler>>,
    telemetry_route_refresh_handler: &StdRwLock<Option<TelemetryRouteRefreshHandler>>,
) -> GatewayForwardOutcome {
    let metrics = GatewayForwardMetrics::default();
    let critical_failure_handler = StdRwLock::new(None);
    let spool = GatewayForwardSpool::new(GatewaySpoolConfig::default());
    let runtime_config = GatewayForwardRuntimeConfig::default();
    let timeouts = StdRwLock::new(GatewayHttpTimeouts {
        connect: Duration::from_secs(1),
        write: Duration::from_secs(1),
        read: Duration::from_secs(1),
        event_post: Duration::from_secs(1),
    });
    post_json_retry_until_expired(
        event,
        "client-a",
        &metrics,
        &critical_failure_handler,
        session_rejection_handler,
        telemetry_route_refresh_handler,
        &spool,
        &runtime_config,
        &timeouts,
        None,
    )
    .await
}

#[tokio::test]
async fn exact_inactive_session_conflict_notifies_the_queued_session_fence_once() {
    let session_id = uuid::Uuid::new_v4();
    let mut event = test_event("/internal/v1/gateway/telemetry", br#"{}"#);
    event.api_url =
        single_response_server("409 Conflict", r#"{"error":"gateway_session_not_active"}"#).await;
    event.gateway_session_id = Some(session_id);
    let rejections = Arc::new(StdMutex::new(Vec::new()));
    let recorded = rejections.clone();
    let handler: GatewaySessionRejectionHandler =
        Arc::new(move |client_id, rejected_session_id| {
            let recorded = recorded.clone();
            Box::pin(async move {
                recorded
                    .lock()
                    .unwrap()
                    .push((client_id, rejected_session_id));
            })
        });
    let handler = StdRwLock::new(Some(handler));

    assert_eq!(
        forward_once(&event, &handler).await,
        GatewayForwardOutcome::NotDelivered
    );
    assert_eq!(
        rejections.lock().unwrap().as_slice(),
        &[("client-a".to_string(), session_id)]
    );
}

#[tokio::test]
async fn inactive_durable_head_requests_private_persistence_without_replay_publication() {
    let dir = std::env::temp_dir().join(format!(
        "vpsman-gateway-private-head-{}",
        uuid::Uuid::new_v4()
    ));
    let ingest = terminal_output_event(
        uuid::Uuid::new_v4(),
        OutputStream::Status,
        Some(1),
        true,
        b"done".to_vec(),
    );
    let mut event = test_event(
        "/internal/v1/gateway/terminal-output",
        &serde_json::to_vec(&ingest).unwrap(),
    );
    event.api_url =
        single_response_server("409 Conflict", r#"{"error":"gateway_session_not_active"}"#).await;
    assert_eq!(
        forward_once(&event, &StdRwLock::new(None)).await,
        GatewayForwardOutcome::NeedsDurableReplay
    );

    mark_spooled_replay_event(&mut event).unwrap();
    event.gateway_session_id = None;
    let spool = GatewayForwardSpool::new(GatewaySpoolConfig::enabled(
        dir.clone(),
        1024 * 1024,
        8 * 1024 * 1024,
        30,
    ));
    let candidate = spool
        .persist_spool_event("client-a", &event, GatewaySpoolPublication::QueueHead)
        .await
        .unwrap();
    assert!(spool.replay_arrivals.lock().unwrap().is_empty());
    assert!(spool.target_has_pending("client-a").await);
    spool
        .remove_spooled_file(&candidate.path, candidate.disk_bytes)
        .await;
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn inactive_durable_head_persist_failure_records_once_without_artifact() {
    let dir = std::env::temp_dir().join(format!(
        "vpsman-gateway-private-head-cap-{}",
        uuid::Uuid::new_v4()
    ));
    let ingest = terminal_output_event(
        uuid::Uuid::new_v4(),
        OutputStream::Status,
        Some(1),
        true,
        b"done".to_vec(),
    );
    let mut event = test_event(
        "/internal/v1/gateway/terminal-output",
        &serde_json::to_vec(&ingest).unwrap(),
    );
    event.api_url =
        single_response_server("409 Conflict", r#"{"error":"gateway_session_not_active"}"#).await;
    let spool = GatewayForwardSpool::new(GatewaySpoolConfig::enabled(
        dir.clone(),
        1024 * 1024,
        1024 * 1024,
        30,
    ));
    spool
        .disk_bytes
        .store(spool.config.disk_max_bytes, Ordering::Relaxed);
    let metrics = GatewayForwardMetrics::default();
    assert_eq!(metrics.try_reserve_queue_slot(), Some(0));
    forward_event_handle(
        "client-a",
        GatewayForwardEventHandle {
            event,
            ram_bytes: 0,
            spool_path: None,
            spool_bytes: 0,
        },
        &metrics,
        &StdRwLock::new(None),
        &StdRwLock::new(None),
        &StdRwLock::new(None),
        &spool,
        &GatewayForwardRuntimeConfig::default(),
        &StdRwLock::new(GatewayHttpTimeouts::default()),
        None,
    )
    .await;
    let snapshot = metrics.snapshot();
    assert_eq!(snapshot.dropped_events, 1);
    assert_eq!(snapshot.dropped_by_reason.protocol_conflict, 1);
    assert_eq!(snapshot.critical_failures, 1);
    assert_eq!(snapshot.critical_failures_by_reason.global_queue_full, 1);
    assert_eq!(snapshot.current_queue_depth, 0);
    assert_eq!(pending_spool_file_count(&dir), 0);
    assert!(spool.replay_arrivals.lock().unwrap().is_empty());
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn inactive_durable_head_retries_before_later_target_fifo_items() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api_url = format!("http://{}", listener.local_addr().unwrap());
    let (head_held_tx, head_held_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let (requests_tx, requests_rx) = oneshot::channel();
    tokio::spawn(async move {
        let mut requests = Vec::new();
        let mut head_held_tx = Some(head_held_tx);
        let mut release_rx = Some(release_rx);
        for request_index in 0..3 {
            let (mut socket, _) = listener.accept().await.unwrap();
            requests.push(read_http_body(&mut socket).await);
            if request_index == 1 {
                head_held_tx.take().unwrap().send(()).unwrap();
                release_rx.take().unwrap().await.unwrap();
            }
            let (status, body) = if request_index == 0 {
                ("409 Conflict", r#"{"error":"gateway_session_not_active"}"#)
            } else {
                ("200 OK", r#"{"accepted":true}"#)
            };
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        }
        requests_tx.send(requests).unwrap();
    });

    let dir = std::env::temp_dir().join(format!(
        "vpsman-gateway-private-head-order-{}",
        uuid::Uuid::new_v4()
    ));
    let forwarder = Arc::new(GatewayEventForwarder::with_spool_config(
        GatewaySpoolConfig::enabled(dir.clone(), 1024 * 1024, 8 * 1024 * 1024, 30),
    ));
    let consumer = forwarder.start_forward_consumers(test_timeouts());
    for seq in [1_u64, 2] {
        let ingest = terminal_output_event(
            uuid::Uuid::new_v4(),
            OutputStream::Status,
            Some(seq),
            true,
            format!("event-{seq}").into_bytes(),
        );
        let mut event = test_event(
            "/internal/v1/gateway/terminal-output",
            &serde_json::to_vec(&ingest).unwrap(),
        );
        event.api_url = api_url.clone();
        event.gateway_session_id = Some(uuid::Uuid::new_v4());
        forwarder
            .enqueue("client-a".to_string(), event, test_timeouts())
            .await
            .unwrap();
    }

    head_held_rx.await.unwrap();
    assert!(forwarder.spool.disk_bytes.load(Ordering::Relaxed) > 0);
    assert!(forwarder.spool.target_has_pending("client-a").await);
    assert!(forwarder.spool.replay_arrivals.lock().unwrap().is_empty());
    release_tx.send(()).unwrap();
    let requests = requests_rx.await.unwrap();
    let requests = requests
        .iter()
        .map(|body| serde_json::from_slice::<GatewayTerminalOutputIngest>(body).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(requests[0].output.terminal_seq, Some(1));
    assert!(!requests[0].spooled_replay);
    assert_eq!(requests[1].output.terminal_seq, Some(1));
    assert!(requests[1].spooled_replay);
    assert_eq!(requests[2].output.terminal_seq, Some(2));

    time::timeout(Duration::from_secs(1), async {
        while forwarder.metrics.delivered_events.load(Ordering::Relaxed) != 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("both exact-target events finish");
    assert_eq!(forwarder.spool.disk_bytes.load(Ordering::Relaxed), 0);
    assert!(!forwarder.spool.target_has_pending("client-a").await);
    assert_eq!(
        forwarder
            .metrics
            .current_queue_depth
            .load(Ordering::Relaxed),
        0
    );
    assert_eq!(forwarder.spool.ram_bytes.load(Ordering::Relaxed), 0);
    forwarder.spool.request_shutdown();
    consumer.await.unwrap();
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn successful_or_unrelated_responses_do_not_reject_the_session_fence() {
    let rejections = Arc::new(StdMutex::new(Vec::new()));
    let recorded = rejections.clone();
    let handler: GatewaySessionRejectionHandler =
        Arc::new(move |client_id, rejected_session_id| {
            let recorded = recorded.clone();
            Box::pin(async move {
                recorded
                    .lock()
                    .unwrap()
                    .push((client_id, rejected_session_id));
            })
        });
    let handler = StdRwLock::new(Some(handler));

    let mut delivered = test_event("/internal/v1/gateway/telemetry", br#"{}"#);
    delivered.api_url = single_response_server("200 OK", r#"{"accepted":true}"#).await;
    delivered.gateway_session_id = Some(uuid::Uuid::new_v4());
    assert_eq!(
        forward_once(&delivered, &handler).await,
        GatewayForwardOutcome::Delivered
    );

    let mut unrelated_conflict = test_event("/internal/v1/gateway/telemetry", br#"{}"#);
    unrelated_conflict.api_url =
        single_response_server("409 Conflict", r#"{"error":"other_conflict"}"#).await;
    unrelated_conflict.gateway_session_id = Some(uuid::Uuid::new_v4());
    unrelated_conflict.created_at = time::Instant::now() - TELEMETRY_EVENT_TTL;
    assert_eq!(
        forward_once(&unrelated_conflict, &handler).await,
        GatewayForwardOutcome::NotDelivered
    );

    let mut server_error = test_event("/internal/v1/gateway/telemetry", br#"{}"#);
    server_error.api_url = single_response_server(
        "500 Internal Server Error",
        r#"{"error":"gateway_session_not_active"}"#,
    )
    .await;
    server_error.gateway_session_id = Some(uuid::Uuid::new_v4());
    server_error.created_at = time::Instant::now() - TELEMETRY_EVENT_TTL;
    assert_eq!(
        forward_once(&server_error, &handler).await,
        GatewayForwardOutcome::NotDelivered
    );

    let mut transport_failure = test_event("/internal/v1/gateway/telemetry", br#"{}"#);
    transport_failure.gateway_session_id = Some(uuid::Uuid::new_v4());
    transport_failure.created_at = time::Instant::now() - TELEMETRY_EVENT_TTL;
    assert_eq!(
        forward_once(&transport_failure, &handler).await,
        GatewayForwardOutcome::NotDelivered
    );
    assert!(rejections.lock().unwrap().is_empty());
}

#[tokio::test]
async fn only_newly_recorded_telemetry_with_an_observed_owner_refreshes_the_exact_route() {
    let session_id = uuid::Uuid::new_v4();
    let observed_owner = GatewayClientDispatchFenceOwner {
        token: uuid::Uuid::new_v4(),
        gateway_epoch: uuid::Uuid::new_v4(),
        generation: 7,
    };
    let refreshes = Arc::new(StdMutex::new(Vec::new()));
    let recorded = refreshes.clone();
    let handler: TelemetryRouteRefreshHandler =
        Arc::new(move |client_id, refreshed_session_id, refreshed_owner| {
            recorded
                .lock()
                .unwrap()
                .push((client_id, refreshed_session_id, refreshed_owner));
            Box::pin(async {})
        });
    let handler = StdRwLock::new(Some(handler));
    let rejections = StdRwLock::new(None);

    let mut recorded_event = test_event("/internal/v1/gateway/telemetry", br#"{}"#);
    recorded_event.api_url = single_response_server(
        "200 OK",
        r#"{"accepted":true,"message":"telemetry_recorded","refresh_route":true}"#,
    )
    .await;
    recorded_event.gateway_session_id = Some(session_id);
    recorded_event.telemetry_route_refresh_owner = Some(Box::new(observed_owner));
    assert_eq!(
        forward_once_with_route_refresh(&recorded_event, &rejections, &handler).await,
        GatewayForwardOutcome::Delivered
    );

    let mut duplicate_event = test_event("/internal/v1/gateway/telemetry", br#"{}"#);
    duplicate_event.api_url = single_response_server(
        "200 OK",
        r#"{"accepted":true,"message":"telemetry_already_recorded","refresh_route":false}"#,
    )
    .await;
    duplicate_event.gateway_session_id = Some(session_id);
    duplicate_event.telemetry_route_refresh_owner = Some(Box::new(observed_owner));
    assert_eq!(
        forward_once_with_route_refresh(&duplicate_event, &rejections, &handler).await,
        GatewayForwardOutcome::Delivered
    );

    let mut unowned_event = test_event("/internal/v1/gateway/telemetry", br#"{}"#);
    unowned_event.api_url = single_response_server(
        "200 OK",
        r#"{"accepted":true,"message":"telemetry_recorded","refresh_route":true}"#,
    )
    .await;
    unowned_event.gateway_session_id = Some(session_id);
    assert_eq!(
        forward_once_with_route_refresh(&unowned_event, &rejections, &handler).await,
        GatewayForwardOutcome::Delivered
    );

    assert_eq!(
        refreshes.lock().unwrap().as_slice(),
        &[("client-a".to_string(), session_id, observed_owner)]
    );
}

#[tokio::test]
async fn shutdown_defers_non_command_events_to_spool() {
    let event = test_event("/internal/v1/gateway/agent-hello", br#"{}"#);
    let metrics = GatewayForwardMetrics::default();
    let critical_failure_handler = StdRwLock::new(None);
    let session_rejection_handler = StdRwLock::new(None);
    let telemetry_route_refresh_handler = StdRwLock::new(None);
    let spool = GatewayForwardSpool::new(GatewaySpoolConfig::default());
    let runtime_config = GatewayForwardRuntimeConfig::default();
    let timeouts = StdRwLock::new(GatewayHttpTimeouts::default());
    spool.request_shutdown();

    let outcome = post_json_retry_until_expired(
        &event,
        "client-a",
        &metrics,
        &critical_failure_handler,
        &session_rejection_handler,
        &telemetry_route_refresh_handler,
        &spool,
        &runtime_config,
        &timeouts,
        None,
    )
    .await;

    assert_eq!(outcome, GatewayForwardOutcome::DeferredForShutdown);
    assert_eq!(metrics.retry_attempts.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn shutdown_interrupts_blocked_api_forward_post() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let Ok((mut socket, _)) = listener.accept().await else {
            return;
        };
        let mut buffer = [0_u8; 1024];
        let _ = socket.read(&mut buffer).await;
        std::future::pending::<()>().await;
    });

    let mut event = test_event("/internal/v1/gateway/agent-hello", br#"{}"#);
    event.api_url = format!("http://{addr}");
    let metrics = GatewayForwardMetrics::default();
    let critical_failure_handler = StdRwLock::new(None);
    let session_rejection_handler = StdRwLock::new(None);
    let telemetry_route_refresh_handler = StdRwLock::new(None);
    let spool = GatewayForwardSpool::new(GatewaySpoolConfig::default());
    let runtime_config = GatewayForwardRuntimeConfig::default();
    let timeouts = StdRwLock::new(GatewayHttpTimeouts {
        connect: Duration::from_secs(1),
        write: Duration::from_secs(1),
        read: Duration::from_secs(60),
        event_post: Duration::from_secs(60),
    });
    let forward = post_json_retry_until_expired(
        &event,
        "client-a",
        &metrics,
        &critical_failure_handler,
        &session_rejection_handler,
        &telemetry_route_refresh_handler,
        &spool,
        &runtime_config,
        &timeouts,
        None,
    );
    tokio::pin!(forward);
    sleep(Duration::from_millis(50)).await;
    spool.request_shutdown();

    let outcome = time::timeout(Duration::from_secs(1), forward)
        .await
        .unwrap();
    assert_eq!(outcome, GatewayForwardOutcome::DeferredForShutdown);
}

#[tokio::test]
async fn post_without_api_url_returns_error() {
    let client = GatewayControlClient::new(None, None, GatewayHttpTimeouts::default());
    let error = client
        .post(
            "client-a",
            "/internal/v1/gateway/agent-hello",
            &serde_json::json!({}),
        )
        .await
        .unwrap_err()
        .to_string();

    assert!(error.contains("gateway API URL is required"));
}

#[tokio::test]
async fn hot_client_telemetry_keeps_one_coalesced_slot_and_one_drain_token() {
    let forwarder = GatewayEventForwarder::default();
    let (sender, mut receiver) = mpsc::channel(PER_TARGET_QUEUE_CAPACITY);
    forwarder.queues.lock().await.insert(
        "client-a".to_string(),
        GatewayForwardQueue {
            sender,
            last_enqueue_unix: unix_now(),
            owner_token: 1,
        },
    );

    forwarder
        .enqueue(
            "client-a".to_string(),
            test_event("/internal/v1/gateway/telemetry", br#"{"seq":1}"#),
            test_timeouts(),
        )
        .await
        .unwrap();
    for sequence in 2..=100 {
        forwarder
            .enqueue(
                "client-a".to_string(),
                test_event(
                    "/internal/v1/gateway/telemetry",
                    format!(r#"{{"seq":{sequence}}}"#).as_bytes(),
                ),
                test_timeouts(),
            )
            .await
            .unwrap();
    }

    let pending = forwarder.telemetry_pending.lock().await;
    assert_eq!(
        pending
            .events
            .get("client-a")
            .map(|event| event.body.as_slice()),
        Some(br#"{"seq":100}"#.as_slice())
    );
    assert_eq!(pending.draining_targets.len(), 1);
    drop(pending);
    assert!(matches!(
        receiver.try_recv(),
        Ok(GatewayForwardQueueItem::Telemetry { .. })
    ));
    assert!(receiver.try_recv().is_err());
    let snapshot = forwarder.metrics.snapshot();
    assert_eq!(snapshot.queued_events, 1);
    assert_eq!(snapshot.current_queue_depth, 1);
    assert_eq!(snapshot.dropped_events, 99);
    assert_eq!(snapshot.telemetry_dropped_events, 99);
    assert_eq!(snapshot.dropped_by_kind.telemetry, 99);
    assert_eq!(snapshot.dropped_by_reason.coalesced, 99);
}

#[tokio::test]
async fn telemetry_http_ownership_is_gateway_bounded_without_losing_queued_work() {
    let forwarder = Arc::new(GatewayEventForwarder::with_config(
        GatewaySpoolConfig::disabled(),
        GatewayForwardConfig::default(),
        2,
    ));
    let consumer_owner = forwarder.start_forward_consumers(test_timeouts());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api_url = format!("http://{}", listener.local_addr().unwrap());
    let (accepted_tx, mut accepted_rx) = mpsc::channel(6);
    tokio::spawn(async move {
        for _ in 0..6 {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 4096];
            let _ = socket.read(&mut request).await;
            accepted_tx.send(socket).await.unwrap();
        }
    });

    for client in 0..6 {
        let mut telemetry = test_event(
            "/internal/v1/gateway/telemetry",
            format!(r#"{{"client":{client}}}"#).as_bytes(),
        );
        telemetry.api_url = api_url.clone();
        forwarder
            .enqueue(format!("client-{client}"), telemetry, test_timeouts())
            .await
            .unwrap();
    }

    let mut open_connections = Vec::new();
    for _ in 0..2 {
        open_connections.push(
            time::timeout(Duration::from_secs(1), accepted_rx.recv())
                .await
                .unwrap()
                .unwrap(),
        );
    }
    time::timeout(Duration::from_secs(1), async {
        loop {
            let snapshot = forwarder.metrics.snapshot();
            if snapshot.telemetry_admission_active == 2 && snapshot.telemetry_admission_waiting == 4
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let ownership_snapshot = forwarder.metrics.snapshot();
    assert_eq!(ownership_snapshot.telemetry_admission_limit, 2);
    assert_eq!(ownership_snapshot.telemetry_admission_active, 2);
    assert_eq!(ownership_snapshot.telemetry_admission_waiting, 4);
    assert!(
        time::timeout(Duration::from_millis(100), accepted_rx.recv())
            .await
            .is_err(),
        "a third telemetry HTTP connection opened while both owners were occupied"
    );

    let response =
        b"HTTP/1.1 200 OK\r\nContent-Length: 17\r\nConnection: close\r\n\r\n{\"accepted\":true}";
    let mut accepted = 2;
    while accepted < 6 {
        let mut socket = open_connections.remove(0);
        socket.write_all(response).await.unwrap();
        socket.shutdown().await.unwrap();
        open_connections.push(
            time::timeout(Duration::from_secs(1), accepted_rx.recv())
                .await
                .unwrap()
                .unwrap(),
        );
        accepted += 1;
    }
    for mut socket in open_connections {
        socket.write_all(response).await.unwrap();
        socket.shutdown().await.unwrap();
    }

    time::timeout(Duration::from_secs(1), async {
        while forwarder.metrics.snapshot().delivered_events != 6
            || forwarder.metrics.snapshot().current_queue_depth != 0
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let completed_snapshot = forwarder.metrics.snapshot();
    assert_eq!(completed_snapshot.dropped_events, 0);
    assert_eq!(completed_snapshot.telemetry_admission_active, 0);
    assert_eq!(completed_snapshot.telemetry_admission_waiting, 0);
    forwarder.shutdown_flush(Duration::from_secs(1)).await;
    consumer_owner.await.unwrap();
}

#[tokio::test]
async fn telemetry_retry_backoff_releases_http_ownership_for_a_healthy_target() {
    let forwarder = Arc::new(GatewayEventForwarder::with_config(
        GatewaySpoolConfig::disabled(),
        GatewayForwardConfig::default(),
        8,
    ));
    let consumer_owner = forwarder.start_forward_consumers(test_timeouts());

    let failing_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let failing_api_url = format!("http://{}", failing_listener.local_addr().unwrap());
    let (failing_sockets_tx, failing_sockets_rx) = oneshot::channel();
    tokio::spawn(async move {
        let mut sockets = Vec::new();
        for _ in 0..8 {
            let (mut socket, _) = failing_listener.accept().await.unwrap();
            let mut request = [0_u8; 4096];
            let _ = socket.read(&mut request).await;
            sockets.push(socket);
        }
        let _ = failing_sockets_tx.send(sockets);
    });

    for client in 0..8 {
        let mut telemetry = test_event(
            "/internal/v1/gateway/telemetry",
            format!(r#"{{"client":{client}}}"#).as_bytes(),
        );
        telemetry.api_url = failing_api_url.clone();
        forwarder
            .enqueue(
                format!("failing-client-{client}"),
                telemetry,
                test_timeouts(),
            )
            .await
            .unwrap();
    }
    let mut failing_sockets = time::timeout(Duration::from_secs(1), failing_sockets_rx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(forwarder.metrics.snapshot().telemetry_admission_active, 8);

    let healthy_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let healthy_api_url = format!("http://{}", healthy_listener.local_addr().unwrap());
    let (healthy_started_tx, healthy_started_rx) = oneshot::channel();
    tokio::spawn(async move {
        let (mut socket, _) = healthy_listener.accept().await.unwrap();
        let mut request = [0_u8; 4096];
        let _ = socket.read(&mut request).await;
        let _ = healthy_started_tx.send(());
        socket
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Length: 17\r\nConnection: close\r\n\r\n{\"accepted\":true}",
            )
            .await
            .unwrap();
        socket.shutdown().await.unwrap();
    });
    let mut healthy = test_event("/internal/v1/gateway/telemetry", br#"{"healthy":true}"#);
    healthy.api_url = healthy_api_url;
    forwarder
        .enqueue("healthy-client".to_string(), healthy, test_timeouts())
        .await
        .unwrap();
    time::timeout(Duration::from_secs(1), async {
        while forwarder.metrics.snapshot().telemetry_admission_waiting != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();

    for socket in &mut failing_sockets {
        socket
            .write_all(
                b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
            )
            .await
            .unwrap();
        socket.shutdown().await.unwrap();
    }

    time::timeout(Duration::from_secs(1), healthy_started_rx)
        .await
        .expect("retrying targets retained every HTTP permit during backoff")
        .unwrap();
    time::timeout(Duration::from_secs(1), async {
        while forwarder.metrics.snapshot().delivered_events != 1 {
            assert!(forwarder.metrics.snapshot().telemetry_admission_active <= 8);
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();

    forwarder.shutdown_flush(Duration::from_secs(1)).await;
    consumer_owner.await.unwrap();
    let snapshot = forwarder.metrics.snapshot();
    assert_eq!(snapshot.telemetry_admission_active, 0);
    assert_eq!(snapshot.telemetry_admission_waiting, 0);
}

#[tokio::test]
async fn telemetry_waits_for_http_ownership_before_taking_the_coalesced_slot() {
    let forwarder = Arc::new(GatewayEventForwarder::with_config(
        GatewaySpoolConfig::disabled(),
        GatewayForwardConfig::default(),
        1,
    ));
    let consumer_owner = forwarder.start_forward_consumers(test_timeouts());
    let held_owner = forwarder
        .telemetry_http_owners
        .clone()
        .acquire_owned()
        .await
        .unwrap();
    let telemetry_api_url = single_response_server("200 OK", r#"{"accepted":true}"#).await;

    for sequence in 1..=2 {
        let mut telemetry = test_event(
            "/internal/v1/gateway/telemetry",
            format!(r#"{{"seq":{sequence}}}"#).as_bytes(),
        );
        telemetry.api_url = telemetry_api_url.clone();
        forwarder
            .enqueue("client-a".to_string(), telemetry, test_timeouts())
            .await
            .unwrap();
    }

    let mut lifecycle = test_event("/internal/v1/gateway/agent-hello", br#"{}"#);
    lifecycle.api_url = single_response_server("200 OK", r#"{"accepted":true}"#).await;
    forwarder
        .enqueue("client-a".to_string(), lifecycle, test_timeouts())
        .await
        .unwrap();
    time::timeout(Duration::from_secs(1), async {
        while forwarder.metrics.snapshot().delivered_events != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();

    let pending = forwarder.telemetry_pending.lock().await;
    assert_eq!(
        pending
            .events
            .get("client-a")
            .map(|event| event.body.as_slice()),
        Some(br#"{"seq":2}"#.as_slice())
    );
    drop(pending);

    drop(held_owner);
    time::timeout(Duration::from_secs(1), async {
        while forwarder.metrics.snapshot().delivered_events != 2
            || forwarder.metrics.snapshot().current_queue_depth != 0
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let snapshot = forwarder.metrics.snapshot();
    assert_eq!(snapshot.dropped_by_reason.coalesced, 1);
    assert!(forwarder.telemetry_pending.lock().await.events.is_empty());
    forwarder.shutdown_flush(Duration::from_secs(1)).await;
    consumer_owner.await.unwrap();
}

#[tokio::test]
async fn one_active_and_one_latest_telemetry_do_not_block_same_client_lifecycle() {
    let forwarder = Arc::new(GatewayEventForwarder::default());
    let consumer_owner = forwarder.start_forward_consumers(test_timeouts());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let blocked_api_url = format!("http://{}", listener.local_addr().unwrap());
    let (request_started_tx, request_started_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = [0_u8; 4096];
        let _ = socket.read(&mut request).await;
        let _ = request_started_tx.send(());
        let _ = release_rx.await;
        socket
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Length: 17\r\nConnection: close\r\n\r\n{\"accepted\":true}",
            )
            .await
            .unwrap();
    });

    let mut telemetry = test_event("/internal/v1/gateway/telemetry", br#"{"seq":1}"#);
    telemetry.api_url = blocked_api_url;
    forwarder
        .enqueue("client-a".to_string(), telemetry, test_timeouts())
        .await
        .unwrap();
    time::timeout(Duration::from_secs(1), request_started_rx)
        .await
        .unwrap()
        .unwrap();

    let mut latest = test_event("/internal/v1/gateway/telemetry", br#"{"seq":2}"#);
    latest.api_url = single_response_server("200 OK", r#"{"accepted":true}"#).await;
    forwarder
        .enqueue("client-a".to_string(), latest, test_timeouts())
        .await
        .unwrap();
    let pending = forwarder.telemetry_pending.lock().await;
    assert_eq!(
        pending
            .events
            .get("client-a")
            .map(|event| event.body.as_slice()),
        Some(br#"{"seq":2}"#.as_slice())
    );
    assert_eq!(pending.draining_targets.len(), 1);
    drop(pending);

    let mut lifecycle = test_event("/internal/v1/gateway/agent-hello", br#"{}"#);
    lifecycle.api_url = single_response_server("200 OK", r#"{"accepted":true}"#).await;
    forwarder
        .enqueue("client-a".to_string(), lifecycle, test_timeouts())
        .await
        .unwrap();
    time::timeout(Duration::from_secs(1), async {
        while forwarder.metrics.snapshot().delivered_events != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(forwarder
        .telemetry_pending
        .lock()
        .await
        .events
        .contains_key("client-a"));

    release_tx.send(()).unwrap();
    time::timeout(Duration::from_secs(1), async {
        while forwarder.metrics.snapshot().delivered_events != 3
            || forwarder.metrics.snapshot().current_queue_depth != 0
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let pending = forwarder.telemetry_pending.lock().await;
    assert!(pending.events.is_empty());
    assert!(pending.draining_targets.is_empty());
    drop(pending);
    forwarder.shutdown_flush(Duration::from_secs(1)).await;
    consumer_owner.await.unwrap();
}

#[tokio::test]
async fn idle_forward_queue_is_retired_only_by_its_exact_consumer_owner() {
    let forwarder = GatewayEventForwarder::default();
    let (sender, _receiver) = mpsc::channel(PER_TARGET_QUEUE_CAPACITY);
    forwarder.queues.lock().await.insert(
        "idle-client".to_string(),
        GatewayForwardQueue {
            sender,
            last_enqueue_unix: unix_now().saturating_sub(QUEUE_IDLE_REAP_SECS + 1),
            owner_token: 7,
        },
    );

    assert!(!retire_forward_queue_if_idle(&forwarder.queues, "idle-client", 8, unix_now(),).await);
    assert!(retire_forward_queue_if_idle(&forwarder.queues, "idle-client", 7, unix_now(),).await);

    let queues = forwarder.queues.lock().await;
    assert!(!queues.contains_key("idle-client"));
}

#[tokio::test]
async fn replayed_event_age_does_not_make_fresh_queue_activity_idle() {
    let forwarder = GatewayEventForwarder::default();
    let (sender, mut receiver) = mpsc::channel(PER_TARGET_QUEUE_CAPACITY);
    forwarder.queues.lock().await.insert(
        "client-a".to_string(),
        GatewayForwardQueue {
            sender,
            last_enqueue_unix: unix_now().saturating_sub(QUEUE_IDLE_REAP_SECS + 1),
            owner_token: 13,
        },
    );
    let mut replay = test_event("/internal/v1/gateway/telemetry", br#"{}"#);
    replay.created_unix = unix_now().saturating_sub(QUEUE_IDLE_REAP_SECS + 60);

    forwarder
        .enqueue_queue_item(
            "client-a".to_string(),
            GatewayForwardQueueItem::Event {
                event: replay,
                ram_bytes: 0,
            },
            test_timeouts(),
        )
        .await
        .unwrap();

    let last_enqueue_unix = forwarder
        .queues
        .lock()
        .await
        .get("client-a")
        .unwrap()
        .last_enqueue_unix;
    assert!(unix_now().saturating_sub(last_enqueue_unix) < QUEUE_IDLE_REAP_SECS);
    assert!(receiver.try_recv().is_ok());
}

#[tokio::test]
async fn closed_forward_consumer_releases_only_its_exact_queue_generation() {
    let forwarder = GatewayEventForwarder::default();
    let (sender, receiver) = mpsc::channel(PER_TARGET_QUEUE_CAPACITY);
    drop(receiver);
    forwarder.queues.lock().await.insert(
        "client-a".to_string(),
        GatewayForwardQueue {
            sender,
            last_enqueue_unix: unix_now(),
            owner_token: 11,
        },
    );

    let result = forwarder
        .enqueue_queue_item(
            "client-a".to_string(),
            GatewayForwardQueueItem::Telemetry {
                created_unix: unix_now(),
                drain_token: 1,
            },
            test_timeouts(),
        )
        .await;

    assert!(result.is_err());
    assert!(!forwarder.queues.lock().await.contains_key("client-a"));
}

#[tokio::test]
async fn telemetry_drain_cleanup_is_fenced_by_its_exact_generation() {
    let pending = Mutex::new(GatewayTelemetryPending::default());
    pending.lock().await.draining_targets.insert(
        "client-a".to_string(),
        GatewayTelemetryDrainOwner {
            token: 17,
            phase: GatewayTelemetryDrainPhase::Queued,
        },
    );

    assert!(!mark_telemetry_drain_running(&pending, "client-a", 16).await);
    assert!(mark_telemetry_drain_running(&pending, "client-a", 17).await);
    assert!(!remove_telemetry_drain_owner(&pending, "client-a", 16).await);
    assert_eq!(
        pending.lock().await.draining_targets.get("client-a"),
        Some(&GatewayTelemetryDrainOwner {
            token: 17,
            phase: GatewayTelemetryDrainPhase::Running,
        })
    );
    assert!(remove_telemetry_drain_owner(&pending, "client-a", 17).await);
    assert!(pending.lock().await.draining_targets.is_empty());
}

#[tokio::test]
async fn consumer_failure_stops_the_main_owned_gateway_health_task() {
    let forwarder = Arc::new(GatewayEventForwarder::default());
    let forward_owner = forwarder.start_forward_consumers(test_timeouts());
    assert!(!forward_owner.is_finished());

    forwarder.consumer_health.fail();
    time::timeout(Duration::from_secs(1), forward_owner)
        .await
        .expect("consumer failure must surface without a retry loop")
        .expect("main-owned health task must exit cleanly");
}

#[tokio::test]
async fn forwarding_shutdown_joins_owned_consumers_and_closes_producer_admission() {
    let forwarder = Arc::new(GatewayEventForwarder::default());
    let forward_owner = forwarder.start_forward_consumers(test_timeouts());
    let mut event = test_event("/internal/v1/gateway/agent-hello", br#"{}"#);
    event.api_url = single_response_server("200 OK", r#"{"accepted":true}"#).await;
    forwarder
        .enqueue("client-a".to_string(), event, test_timeouts())
        .await
        .unwrap();
    time::timeout(Duration::from_secs(1), async {
        while forwarder.metrics.snapshot().current_queue_depth != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();

    forwarder.shutdown_flush(Duration::from_secs(1)).await;
    time::timeout(Duration::from_secs(1), forward_owner)
        .await
        .expect("the main-owned forwarding runtime must drain")
        .expect("the main-owned forwarding runtime must join cleanly");
    assert!(forwarder.queues.lock().await.is_empty());
    assert!(forwarder
        .enqueue(
            "client-a".to_string(),
            test_event("/internal/v1/gateway/agent-hello", br#"{}"#),
            test_timeouts(),
        )
        .await
        .is_err());
    assert_eq!(forwarder.metrics.snapshot().current_queue_depth, 0);
}

#[tokio::test]
async fn forwarding_shutdown_joins_an_in_flight_exact_telemetry_drain() {
    let forwarder = Arc::new(GatewayEventForwarder::default());
    let forward_owner = forwarder.start_forward_consumers(test_timeouts());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut event = test_event("/internal/v1/gateway/telemetry", br#"{}"#);
    event.api_url = format!("http://{}", listener.local_addr().unwrap());
    forwarder
        .enqueue("client-a".to_string(), event, test_timeouts())
        .await
        .unwrap();
    let (mut socket, _) = time::timeout(Duration::from_secs(1), listener.accept())
        .await
        .unwrap()
        .unwrap();
    let mut request = [0_u8; 4096];
    time::timeout(Duration::from_secs(1), socket.read(&mut request))
        .await
        .unwrap()
        .unwrap();

    forwarder.shutdown_flush(Duration::from_millis(1)).await;
    time::timeout(Duration::from_secs(1), forward_owner)
        .await
        .expect("the in-flight telemetry drain must observe shutdown")
        .expect("the in-flight telemetry drain must be joined by its owner");
    assert!(forwarder.queues.lock().await.is_empty());
    let pending = forwarder.telemetry_pending.lock().await;
    assert!(pending.events.is_empty());
    assert!(pending.draining_targets.is_empty());
}

#[tokio::test]
async fn durable_replay_progresses_for_an_exact_ready_target_during_live_queue_activity() {
    let dir = std::env::temp_dir().join(format!(
        "vpsman-gateway-spool-live-progress-{}",
        uuid::Uuid::new_v4()
    ));
    let forwarder = GatewayEventForwarder::with_spool_config(GatewaySpoolConfig::enabled(
        dir.clone(),
        1024 * 1024,
        8 * 1024 * 1024,
        30,
    ));
    let blocked_first = test_event("/internal/v1/gateway/command-output", br#"{"seq":1}"#);
    let blocked_first_seq = blocked_first.enqueue_seq;
    let blocked_item = forwarder
        .spool
        .spool_event("client-blocked", &blocked_first)
        .await
        .unwrap();
    let blocked_later_item = forwarder
        .spool
        .spool_event(
            "client-blocked",
            &test_event("/internal/v1/gateway/command-output", br#"{"seq":2}"#),
        )
        .await
        .unwrap();
    let ready_item = forwarder
        .spool
        .spool_event(
            "client-ready",
            &test_event("/internal/v1/gateway/command-output", br#"{"seq":3}"#),
        )
        .await
        .unwrap();
    drop((blocked_item, blocked_later_item, ready_item));

    let (blocked_tx, mut blocked_rx) = mpsc::channel(1);
    blocked_tx
        .try_send(GatewayForwardQueueItem::Event {
            event: test_event("/internal/v1/gateway/agent-hello", br#"{}"#),
            ram_bytes: 0,
        })
        .unwrap();
    let (ready_tx, mut ready_rx) = mpsc::channel(1);
    {
        let mut queues = forwarder.queues.lock().await;
        queues.insert(
            "client-blocked".to_string(),
            GatewayForwardQueue {
                sender: blocked_tx,
                last_enqueue_unix: unix_now(),
                owner_token: 1,
            },
        );
        queues.insert(
            "client-ready".to_string(),
            GatewayForwardQueue {
                sender: ready_tx,
                last_enqueue_unix: unix_now(),
                owner_token: 2,
            },
        );
    }
    // Represents unrelated continuous live traffic. Durable replay eligibility
    // must not depend on the fleet-wide queue depth becoming zero.
    forwarder
        .metrics
        .current_queue_depth
        .store(100, Ordering::Relaxed);
    let mut replay_index = GatewaySpoolReplayIndex::default();
    assert!(
        forwarder
            .replay_pending_spool_once(&mut replay_index, test_timeouts())
            .await
    );
    assert!(replay_index.blocked_targets.contains("client-blocked"));
    assert!(matches!(
        ready_rx.try_recv(),
        Ok(GatewayForwardQueueItem::Spooled { .. })
    ));
    assert!(blocked_rx.try_recv().is_ok());
    forwarder.spool.mark_replay_target_ready("client-blocked");
    assert!(
        forwarder
            .replay_pending_spool_once(&mut replay_index, test_timeouts())
            .await
    );
    let replayed_first = blocked_rx.try_recv().unwrap();
    assert!(matches!(
        replayed_first,
        GatewayForwardQueueItem::Spooled { enqueue_seq, .. }
            if enqueue_seq == blocked_first_seq
    ));

    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn replay_backlog_is_indexed_once_ordered_and_fair_across_queue_wakes() {
    let forwarder = GatewayEventForwarder::with_spool_config(GatewaySpoolConfig::enabled(
        std::env::temp_dir().join(format!(
            "vpsman-gateway-replay-index-{}",
            uuid::Uuid::new_v4()
        )),
        1024 * 1024,
        8 * 1024 * 1024,
        30,
    ));
    let backlog = PER_TARGET_QUEUE_CAPACITY as u64 + 2;
    {
        let mut arrivals = forwarder.spool.replay_arrivals.lock().unwrap();
        for enqueue_seq in 1..=backlog {
            arrivals.push(synthetic_spool_candidate("client-a", enqueue_seq));
        }
        arrivals.push(synthetic_spool_candidate("client-b", 10_000));
    }
    let (sender_a, mut receiver_a) = mpsc::channel(PER_TARGET_QUEUE_CAPACITY);
    let (sender_b, mut receiver_b) = mpsc::channel(1);
    {
        let mut queues = forwarder.queues.lock().await;
        queues.insert(
            "client-a".to_string(),
            GatewayForwardQueue {
                sender: sender_a,
                last_enqueue_unix: unix_now(),
                owner_token: 1,
            },
        );
        queues.insert(
            "client-b".to_string(),
            GatewayForwardQueue {
                sender: sender_b,
                last_enqueue_unix: unix_now(),
                owner_token: 2,
            },
        );
    }

    let mut index = GatewaySpoolReplayIndex::default();
    assert!(
        forwarder
            .replay_pending_spool_once(&mut index, test_timeouts())
            .await
    );
    assert_eq!(receiver_b.try_recv().unwrap().enqueue_seq(), 10_000);
    assert_eq!(receiver_a.try_recv().unwrap().enqueue_seq(), 1);
    forwarder.spool.mark_replay_target_ready("client-a");
    assert!(
        forwarder
            .replay_pending_spool_once(&mut index, test_timeouts())
            .await
    );
    for enqueue_seq in 2..=PER_TARGET_QUEUE_CAPACITY as u64 + 1 {
        assert_eq!(receiver_a.try_recv().unwrap().enqueue_seq(), enqueue_seq);
    }
    forwarder.spool.mark_replay_target_ready("client-a");
    assert!(
        forwarder
            .replay_pending_spool_once(&mut index, test_timeouts())
            .await
    );
    assert_eq!(receiver_a.try_recv().unwrap().enqueue_seq(), backlog);
    assert_eq!(
        forwarder.spool.replay_header_reads.load(Ordering::Relaxed),
        0
    );
    assert_eq!(forwarder.spool.replay_body_reads.load(Ordering::Relaxed), 0);
    let metrics = forwarder.metrics.snapshot();
    assert_eq!(metrics.dropped_events, 0);
    assert_eq!(metrics.critical_failures, 0);
}

#[tokio::test]
async fn replay_obeys_the_global_queue_cap_and_resumes_on_the_exact_crossing() {
    let forwarder = GatewayEventForwarder::default();
    forwarder
        .metrics
        .current_queue_depth
        .store(GLOBAL_QUEUE_CAPACITY, Ordering::Relaxed);
    forwarder
        .spool
        .replay_arrivals
        .lock()
        .unwrap()
        .push(synthetic_spool_candidate("client-a", 1));
    let (sender, mut receiver) = mpsc::channel(1);
    forwarder.queues.lock().await.insert(
        "client-a".to_string(),
        GatewayForwardQueue {
            sender,
            last_enqueue_unix: unix_now(),
            owner_token: 1,
        },
    );
    let mut index = GatewaySpoolReplayIndex::default();
    assert!(
        !forwarder
            .replay_pending_spool_once(&mut index, test_timeouts())
            .await
    );
    assert!(receiver.try_recv().is_err());

    finish_forward_event(&forwarder.metrics, &forwarder.spool, None, false).await;
    assert!(
        forwarder
            .replay_pending_spool_once(&mut index, test_timeouts())
            .await
    );
    assert_eq!(receiver.try_recv().unwrap().enqueue_seq(), 1);
    assert_eq!(
        forwarder
            .metrics
            .current_queue_depth
            .load(Ordering::Relaxed),
        GLOBAL_QUEUE_CAPACITY
    );
    assert_eq!(forwarder.metrics.snapshot().critical_failures, 0);
}

#[tokio::test]
async fn critical_enqueue_overflow_marks_unhealthy_and_notifies_handler() {
    let forwarder = GatewayEventForwarder::default();
    forwarder
        .metrics
        .current_queue_depth
        .store(GLOBAL_QUEUE_CAPACITY, Ordering::Relaxed);
    let (sent, received) = oneshot::channel::<(String, &'static str)>();
    let sent = std::sync::Mutex::new(Some(sent));
    *forwarder.critical_failure_handler.write().unwrap() =
        Some(Arc::new(move |client_id, reason| {
            let sender = sent.lock().unwrap().take();
            Box::pin(async move {
                if let Some(sender) = sender {
                    let _ = sender.send((client_id, reason));
                }
            })
        }));

    let result = forwarder
        .enqueue(
            "client-a".to_string(),
            test_event("/internal/v1/gateway/command-output", br#"{}"#),
            test_timeouts(),
        )
        .await;

    assert!(result.is_err());
    let (client_id, reason) = received.await.unwrap();
    assert_eq!(client_id, "client-a");
    assert_eq!(reason, "global_queue_full");
    let snapshot = forwarder.metrics.snapshot();
    assert!(snapshot.unhealthy);
    assert_eq!(snapshot.critical_failures, 1);
    assert_eq!(snapshot.critical_failures_by_reason.global_queue_full, 1);
    assert_eq!(snapshot.dropped_by_kind.command_output, 1);
}

#[tokio::test]
async fn command_output_over_ram_budget_spools_to_disk() {
    let dir = std::env::temp_dir().join(format!("vpsman-gateway-spool-{}", uuid::Uuid::new_v4()));
    let forwarder = GatewayEventForwarder::with_spool_config(GatewaySpoolConfig::enabled(
        dir.clone(),
        1024 * 1024,
        8 * 1024 * 1024,
        30,
    ));
    let body = vec![b'x'; 1024 * 1024 + 1];
    let event = test_event("/internal/v1/gateway/command-output", &body);

    let item = forwarder
        .prepare_queue_item("client-a", event)
        .await
        .unwrap();

    assert!(
        item.is_none(),
        "command output above RAM budget should spool"
    );
    let mut replay_index = GatewaySpoolReplayIndex::default();
    let replay = take_replay_items(&forwarder, &mut replay_index).await;
    let GatewayForwardQueueItem::Spooled {
        path, disk_bytes, ..
    } = &replay[0].1
    else {
        panic!("durable command output must enter replay");
    };
    assert!(path.exists());
    assert!(*disk_bytes > body.len() as u64);
    assert_eq!(mode(&dir), 0o700);
    assert_eq!(mode(&dir.join("pending")), 0o700);
    assert_eq!(mode(path), 0o600);
    let decoded = forwarder.spool.load_spooled_event(path).await.unwrap();
    assert_eq!(decoded.body, body);
    assert_eq!(decoded.kind, GatewayForwardEventKind::CommandOutput);
    assert!(decoded.critical);
    forwarder.spool.remove_spooled_file(path, *disk_bytes).await;
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn terminal_output_over_ram_budget_spools_to_disk() {
    let dir = std::env::temp_dir().join(format!(
        "vpsman-gateway-terminal-spool-{}",
        uuid::Uuid::new_v4()
    ));
    let forwarder = GatewayEventForwarder::with_spool_config(GatewaySpoolConfig::enabled(
        dir.clone(),
        128,
        8 * 1024 * 1024,
        30,
    ));
    let job_id = uuid::Uuid::new_v4();
    let event = terminal_output_event(
        job_id,
        vpsman_common::OutputStream::Pty,
        Some(1),
        false,
        vec![0_u8; 1024 * 1024 + 1],
    );
    let body = serde_json::to_vec(&event).unwrap();
    assert!(body.len() as u64 > forwarder.spool.config.ram_max_bytes);
    let item = forwarder
        .prepare_queue_item(
            "client-a",
            test_event("/internal/v1/gateway/terminal-output", &body),
        )
        .await
        .unwrap();

    assert!(
        item.is_none(),
        "terminal output above RAM budget should spool"
    );
    let mut replay_index = GatewaySpoolReplayIndex::default();
    let replay = take_replay_items(&forwarder, &mut replay_index).await;
    let GatewayForwardQueueItem::Spooled {
        path, disk_bytes, ..
    } = &replay[0].1
    else {
        panic!("durable terminal output must enter replay");
    };
    let decoded = forwarder.spool.load_spooled_event(path).await.unwrap();
    assert_eq!(decoded.kind, GatewayForwardEventKind::TerminalOutput);
    assert!(!decoded.critical);
    forwarder.spool.remove_spooled_file(path, *disk_bytes).await;
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn terminal_final_status_is_critical_with_command_output_ttl() {
    let job_id = uuid::Uuid::new_v4();
    let final_status = terminal_output_event(
        job_id,
        vpsman_common::OutputStream::Status,
        None,
        true,
        br#"{"type":"terminal_stream","status":"exited"}"#.to_vec(),
    );
    let body = serde_json::to_vec(&final_status).unwrap();
    let event = test_event("/internal/v1/gateway/terminal-output", &body);

    assert!(event.critical);
    assert_eq!(
        event.ttl(&GatewayForwardRuntimeConfig::new(
            GatewayForwardConfig::new(900)
        )),
        Duration::from_secs(900)
    );
}

#[tokio::test]
async fn command_output_spool_header_preserves_command_output_replay_key() {
    let dir = std::env::temp_dir().join(format!(
        "vpsman-gateway-spool-header-{}",
        uuid::Uuid::new_v4()
    ));
    let forwarder = GatewayEventForwarder::with_spool_config(GatewaySpoolConfig::enabled(
        dir.clone(),
        1024 * 1024,
        8 * 1024 * 1024,
        30,
    ));
    let job_id = uuid::Uuid::new_v4();
    let ingest = GatewayCommandOutputIngest {
        gateway_id: "gateway-a".to_string(),
        gateway_session_id: uuid::Uuid::new_v4(),
        process_incarnation_id: uuid::Uuid::new_v4(),
        spooled_replay: false,
        client_id: "client-a".to_string(),
        job_id,
        payload_hash: "payload-a".to_string(),
        seq: 7,
        received_unix: Some(unix_now()),
        output: vpsman_common::CommandOutput {
            job_id,
            stream: vpsman_common::OutputStream::Status,
            data: br#"{"type":"ok"}"#.to_vec(),
            exit_code: Some(0),
            done: true,
        },
    };
    let replay_key = CommandOutputReplayRef::from(&ingest);
    let mut event = test_event(
        COMMAND_OUTPUT_PATH,
        &serde_json::to_vec(&ingest).expect("serialize ingest"),
    );
    event.command_output = Some(replay_key.clone());

    let candidate = forwarder
        .spool
        .spool_event("client-a", &event)
        .await
        .unwrap();

    let header = forwarder
        .spool
        .load_spooled_header_sync(&candidate.path)
        .unwrap();
    assert_eq!(header.command_output, Some(replay_key));
    assert!(header.critical);
    forwarder
        .spool
        .remove_spooled_file(&candidate.path, candidate.disk_bytes)
        .await;
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn delivered_spool_unlink_failure_is_cleaned_by_replay_without_redelivery() {
    let dir = std::env::temp_dir().join(format!(
        "vpsman-gateway-delivered-cleanup-{}",
        uuid::Uuid::new_v4()
    ));
    let forwarder = GatewayEventForwarder::with_spool_config(GatewaySpoolConfig::enabled(
        dir.clone(),
        1024 * 1024,
        8 * 1024 * 1024,
        30,
    ));
    let event = test_event(COMMAND_OUTPUT_PATH, br#"{"seq":1}"#);
    let candidate = forwarder
        .spool
        .persist_spool_event("client-a", &event, GatewaySpoolPublication::QueueHead)
        .await
        .unwrap();
    let accounted = forwarder.spool.disk_bytes.load(Ordering::Relaxed);
    forwarder
        .spool
        .delivered_cleanup_remove_failures
        .store(1, Ordering::Relaxed);
    forwarder
        .metrics
        .current_queue_depth
        .store(1, Ordering::Relaxed);

    finish_forward_event(
        &forwarder.metrics,
        &forwarder.spool,
        Some(&GatewayForwardEventHandle {
            event,
            ram_bytes: 0,
            spool_path: Some(candidate.path.clone()),
            spool_bytes: candidate.disk_bytes,
        }),
        false,
    )
    .await;

    assert!(candidate.path.exists());
    assert_eq!(
        forwarder.spool.disk_bytes.load(Ordering::Relaxed),
        accounted
    );
    assert!(forwarder.spool.target_has_pending("client-a").await);
    assert_eq!(
        forwarder
            .spool
            .delivered_cleanup_remove_attempts
            .load(Ordering::Relaxed),
        1
    );
    {
        let retries = forwarder.spool.delivered_cleanup_retries.lock().unwrap();
        assert_eq!(retries.entries.len(), 1);
        assert_eq!(retries.ready.len(), 1);
        assert!(retries.dormant.is_empty());
    }

    let mut replay_index = GatewaySpoolReplayIndex::default();
    assert!(!forwarder
        .spool
        .refresh_replay_index(&mut replay_index)
        .await
        .unwrap());
    assert!(!candidate.path.exists());
    assert_eq!(forwarder.spool.disk_bytes.load(Ordering::Relaxed), 0);
    assert!(!forwarder.spool.target_has_pending("client-a").await);
    assert!(replay_index.pop_next().is_none());
    assert!(forwarder
        .spool
        .delivered_cleanup_retries
        .lock()
        .unwrap()
        .entries
        .is_empty());
    assert_eq!(
        forwarder
            .spool
            .delivered_cleanup_remove_attempts
            .load(Ordering::Relaxed),
        2
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn persistent_delivered_unlink_failure_waits_for_distinct_replay_activity() {
    let dir = std::env::temp_dir().join(format!(
        "vpsman-gateway-delivered-cleanup-dormant-{}",
        uuid::Uuid::new_v4()
    ));
    let forwarder = GatewayEventForwarder::with_spool_config(GatewaySpoolConfig::enabled(
        dir.clone(),
        1024 * 1024,
        8 * 1024 * 1024,
        30,
    ));
    let event = test_event(COMMAND_OUTPUT_PATH, br#"{"seq":1}"#);
    let candidate = forwarder
        .spool
        .persist_spool_event("client-a", &event, GatewaySpoolPublication::QueueHead)
        .await
        .unwrap();
    forwarder
        .spool
        .delivered_cleanup_remove_failures
        .store(3, Ordering::Relaxed);
    forwarder
        .metrics
        .current_queue_depth
        .store(1, Ordering::Relaxed);
    finish_forward_event(
        &forwarder.metrics,
        &forwarder.spool,
        Some(&GatewayForwardEventHandle {
            event,
            ram_bytes: 0,
            spool_path: Some(candidate.path.clone()),
            spool_bytes: candidate.disk_bytes,
        }),
        false,
    )
    .await;

    let mut replay_index = GatewaySpoolReplayIndex::default();
    assert!(!forwarder
        .spool
        .refresh_replay_index(&mut replay_index)
        .await
        .unwrap());
    assert_eq!(
        forwarder
            .spool
            .delivered_cleanup_remove_attempts
            .load(Ordering::Relaxed),
        2
    );
    assert!(!forwarder
        .spool
        .refresh_replay_index(&mut replay_index)
        .await
        .unwrap());
    assert_eq!(
        forwarder
            .spool
            .delivered_cleanup_remove_attempts
            .load(Ordering::Relaxed),
        2
    );

    forwarder.spool.watch_replay_target("wake-target");
    forwarder.spool.mark_replay_target_ready("wake-target");
    assert!(!forwarder
        .spool
        .refresh_replay_index(&mut replay_index)
        .await
        .unwrap());
    assert_eq!(
        forwarder
            .spool
            .delivered_cleanup_remove_attempts
            .load(Ordering::Relaxed),
        3
    );
    assert!(!forwarder
        .spool
        .refresh_replay_index(&mut replay_index)
        .await
        .unwrap());
    assert_eq!(
        forwarder
            .spool
            .delivered_cleanup_remove_attempts
            .load(Ordering::Relaxed),
        3
    );
    {
        let retries = forwarder.spool.delivered_cleanup_retries.lock().unwrap();
        assert_eq!(retries.entries.len(), 1);
        assert!(retries.ready.is_empty());
        assert_eq!(retries.dormant.len(), 1);
    }

    forwarder.spool.watch_replay_target("wake-target");
    forwarder.spool.mark_replay_target_ready("wake-target");
    assert!(!forwarder
        .spool
        .refresh_replay_index(&mut replay_index)
        .await
        .unwrap());
    assert!(!candidate.path.exists());
    assert_eq!(forwarder.spool.disk_bytes.load(Ordering::Relaxed), 0);
    assert!(!forwarder.spool.target_has_pending("client-a").await);
    assert!(replay_index.pop_next().is_none());
    assert!(forwarder
        .spool
        .delivered_cleanup_retries
        .lock()
        .unwrap()
        .entries
        .is_empty());
    assert_eq!(
        forwarder
            .spool
            .delivered_cleanup_remove_attempts
            .load(Ordering::Relaxed),
        4
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn full_disk_reservation_reactivates_one_dormant_delivered_cleanup() {
    let dir = std::env::temp_dir().join(format!(
        "vpsman-gateway-delivered-cleanup-disk-cap-{}",
        uuid::Uuid::new_v4()
    ));
    let forwarder = GatewayEventForwarder::with_spool_config(GatewaySpoolConfig::enabled(
        dir.clone(),
        1024 * 1024,
        8 * 1024 * 1024,
        30,
    ));
    let mut delivered = test_event(COMMAND_OUTPUT_PATH, br#"{"seq":1}"#);
    delivered.created_unix = 100;
    delivered.enqueue_seq = 100;
    let candidate = forwarder
        .spool
        .persist_spool_event("client-a", &delivered, GatewaySpoolPublication::QueueHead)
        .await
        .unwrap();
    let disk_cap = forwarder.spool.config.disk_max_bytes;
    // The remainder models other independently accounted active spool files;
    // deleting this delivered file frees exactly enough room for its successor.
    forwarder
        .spool
        .disk_bytes
        .store(disk_cap, Ordering::Relaxed);
    forwarder
        .spool
        .delivered_cleanup_remove_failures
        .store(2, Ordering::Relaxed);
    forwarder
        .metrics
        .current_queue_depth
        .store(1, Ordering::Relaxed);
    finish_forward_event(
        &forwarder.metrics,
        &forwarder.spool,
        Some(&GatewayForwardEventHandle {
            event: delivered,
            ram_bytes: 0,
            spool_path: Some(candidate.path.clone()),
            spool_bytes: candidate.disk_bytes,
        }),
        false,
    )
    .await;
    let mut replay_index = GatewaySpoolReplayIndex::default();
    assert!(!forwarder
        .spool
        .refresh_replay_index(&mut replay_index)
        .await
        .unwrap());
    assert_eq!(
        forwarder
            .spool
            .delivered_cleanup_retries
            .lock()
            .unwrap()
            .dormant
            .len(),
        1
    );

    let mut next = test_event(COMMAND_OUTPUT_PATH, br#"{"seq":1}"#);
    next.created_unix = 100;
    next.enqueue_seq = 101;
    let error = forwarder
        .spool
        .persist_spool_event("client-a", &next, GatewaySpoolPublication::QueueHead)
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("gateway spool disk cap exceeded"));
    {
        let retries = forwarder.spool.delivered_cleanup_retries.lock().unwrap();
        assert_eq!(retries.ready.len(), 1);
        assert!(retries.dormant.is_empty());
    }
    assert!(!forwarder
        .spool
        .refresh_replay_index(&mut replay_index)
        .await
        .unwrap());
    assert!(!candidate.path.exists());
    assert!(!forwarder.spool.target_has_pending("client-a").await);
    assert_eq!(
        forwarder.spool.disk_bytes.load(Ordering::Relaxed),
        disk_cap - candidate.disk_bytes
    );

    let next_candidate = forwarder
        .spool
        .persist_spool_event("client-a", &next, GatewaySpoolPublication::QueueHead)
        .await
        .unwrap();
    assert_eq!(next_candidate.disk_bytes, candidate.disk_bytes);
    assert_eq!(forwarder.spool.disk_bytes.load(Ordering::Relaxed), disk_cap);
    forwarder
        .spool
        .remove_spooled_file(&next_candidate.path, next_candidate.disk_bytes)
        .await;
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn full_disk_without_dormant_cleanup_does_not_signal_replay() {
    let dir = std::env::temp_dir().join(format!(
        "vpsman-gateway-full-disk-no-cleanup-{}",
        uuid::Uuid::new_v4()
    ));
    let spool =
        GatewayForwardSpool::new(GatewaySpoolConfig::enabled(dir.clone(), 1024 * 1024, 1, 30));
    spool
        .disk_bytes
        .store(spool.config.disk_max_bytes, Ordering::Relaxed);

    assert!(spool.try_reserve_disk(1).is_err());
    assert!(spool.try_reserve_disk(1).is_err());
    assert!(spool
        .delivered_cleanup_retries
        .lock()
        .unwrap()
        .entries
        .is_empty());
    assert_eq!(
        spool
            .delivered_cleanup_remove_attempts
            .load(Ordering::Relaxed),
        0
    );
    let mut replay_ready = Box::pin(spool.replay_ready.notified());
    std::future::poll_fn(|context| {
        assert!(replay_ready.as_mut().poll(context).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn disk_activity_during_in_flight_cleanup_records_one_reactivation() {
    let dir = std::env::temp_dir().join(format!(
        "vpsman-gateway-cleanup-in-flight-wake-{}",
        uuid::Uuid::new_v4()
    ));
    let spool = GatewayForwardSpool::new(GatewaySpoolConfig::enabled(
        dir.clone(),
        1024 * 1024,
        1024 * 1024,
        30,
    ));
    let path = dir.join("pending").join("delivered.spool");
    {
        let mut retries = spool.delivered_cleanup_retries.lock().unwrap();
        assert!(retries.insert_ready(path.clone(), 1));
        assert_eq!(retries.take_ready(), Some((path.clone(), 1)));
        assert_eq!(retries.in_flight.as_deref(), Some(path.as_path()));
    }
    spool
        .disk_bytes
        .store(spool.config.disk_max_bytes, Ordering::Relaxed);

    assert!(spool.try_reserve_disk(1).is_err());
    assert!(spool.try_reserve_disk(1).is_err());
    {
        let retries = spool.delivered_cleanup_retries.lock().unwrap();
        assert!(retries.in_flight_reactivation);
        assert!(retries.ready.is_empty());
        assert!(retries.dormant.is_empty());
    }
    spool.replay_ready.notified().await;
    let mut no_second_wake = Box::pin(spool.replay_ready.notified());
    std::future::poll_fn(|context| {
        assert!(no_second_wake.as_mut().poll(context).is_pending());
        std::task::Poll::Ready(())
    })
    .await;

    {
        let mut retries = spool.delivered_cleanup_retries.lock().unwrap();
        assert!(retries.complete(&path, false));
        assert_eq!(retries.ready.len(), 1);
        assert!(retries.dormant.is_empty());
        assert!(!retries.in_flight_reactivation);
        assert_eq!(retries.take_ready(), Some((path.clone(), 1)));
        assert!(retries.complete(&path, false));
        assert!(retries.ready.is_empty());
        assert_eq!(retries.dormant.len(), 1);
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn spool_writer_rejects_headers_that_restart_reader_cannot_admit() {
    let dir = std::env::temp_dir().join(format!(
        "vpsman-gateway-spool-header-limit-{}",
        uuid::Uuid::new_v4()
    ));
    let forwarder = GatewayEventForwarder::with_spool_config(GatewaySpoolConfig::enabled(
        dir.clone(),
        1024 * 1024,
        8 * 1024 * 1024,
        30,
    ));
    let mut event = test_event(COMMAND_OUTPUT_PATH, br#"{"seq":1}"#);
    event.internal_token = Some("x".repeat(MAX_SPOOL_HEADER_BYTES + 1));

    let error = forwarder
        .spool
        .spool_event("client-a", &event)
        .await
        .unwrap_err()
        .to_string();

    assert!(error.contains("header exceeds limit"));
    assert_eq!(forwarder.spool.disk_bytes.load(Ordering::Relaxed), 0);
    assert_eq!(pending_spool_file_count(&dir), 0);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn command_output_spool_replay_marker_sets_body_field() {
    let job_id = uuid::Uuid::new_v4();
    let ingest = GatewayCommandOutputIngest {
        gateway_id: "gateway-a".to_string(),
        gateway_session_id: uuid::Uuid::new_v4(),
        process_incarnation_id: uuid::Uuid::new_v4(),
        spooled_replay: false,
        client_id: "client-a".to_string(),
        job_id,
        payload_hash: "a".repeat(64),
        seq: 7,
        received_unix: Some(unix_now()),
        output: vpsman_common::CommandOutput {
            job_id,
            stream: vpsman_common::OutputStream::Status,
            data: br#"{"type":"ok"}"#.to_vec(),
            exit_code: Some(0),
            done: true,
        },
    };
    let mut event = test_event(
        COMMAND_OUTPUT_PATH,
        &serde_json::to_vec(&ingest).expect("serialize ingest"),
    );

    mark_spooled_replay_event(&mut event).unwrap();

    let marked: GatewayCommandOutputIngest = serde_json::from_slice(&event.body).unwrap();
    assert!(marked.spooled_replay);
    assert!(event_marked_spooled_replay(&event));
    assert_eq!(
        event.command_output,
        Some(CommandOutputReplayRef::from(&marked))
    );
}

#[tokio::test]
async fn full_target_queue_preserves_spooled_command_output_file() {
    let dir = std::env::temp_dir().join(format!(
        "vpsman-gateway-spool-pressure-{}",
        uuid::Uuid::new_v4()
    ));
    let forwarder = GatewayEventForwarder::with_spool_config(GatewaySpoolConfig::enabled(
        dir.clone(),
        1024 * 1024,
        8 * 1024 * 1024,
        30,
    ));
    let (sender, _receiver) = mpsc::channel(1);
    sender
        .try_send(GatewayForwardQueueItem::Telemetry {
            created_unix: unix_now(),
            drain_token: 1,
        })
        .unwrap();
    forwarder.queues.lock().await.insert(
        "client-a".to_string(),
        GatewayForwardQueue {
            sender,
            last_enqueue_unix: unix_now(),
            owner_token: 1,
        },
    );
    let event = test_event(COMMAND_OUTPUT_PATH, br#"{"seq":1}"#);
    let candidate = forwarder
        .spool
        .spool_event("client-a", &event)
        .await
        .unwrap();
    let path = candidate.path.clone();
    let disk_bytes = candidate.disk_bytes;
    let accounted_before = forwarder.spool.disk_bytes.load(Ordering::Relaxed);
    let failure_notifications = Arc::new(AtomicU64::new(0));
    let recorded = failure_notifications.clone();
    *forwarder.critical_failure_handler.write().unwrap() = Some(Arc::new(move |_, _| {
        recorded.fetch_add(1, Ordering::Relaxed);
        Box::pin(async {})
    }));

    let result = forwarder
        .enqueue_queue_item(
            "client-a".to_string(),
            candidate.into_queue_item(),
            test_timeouts(),
        )
        .await;

    assert!(result.is_err());
    assert!(path.exists());
    assert_eq!(
        forwarder.spool.disk_bytes.load(Ordering::Relaxed),
        accounted_before
    );
    let metrics = forwarder.metrics.snapshot();
    assert_eq!(metrics.dropped_events, 0);
    assert_eq!(metrics.critical_failures, 0);
    assert_eq!(failure_notifications.load(Ordering::Relaxed), 0);
    forwarder.spool.remove_spooled_file(&path, disk_bytes).await;
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn pressure_spooled_output_counts_against_disk_cap() {
    let dir = std::env::temp_dir().join(format!(
        "vpsman-gateway-spool-disk-cap-{}",
        uuid::Uuid::new_v4()
    ));
    let forwarder = GatewayEventForwarder::with_spool_config(GatewaySpoolConfig::enabled(
        dir.clone(),
        1024 * 1024,
        1024 * 1024,
        30,
    ));
    let event = test_event(COMMAND_OUTPUT_PATH, &vec![b'x'; 700 * 1024]);

    spool_event_for_later_replay(
        &forwarder.spool,
        "client-a",
        &event,
        GatewayForwardDropReason::GlobalQueueFull,
    )
    .await
    .unwrap();
    assert_eq!(pending_spool_file_count(&dir), 1);
    assert!(forwarder.spool.disk_bytes.load(Ordering::Relaxed) > 0);

    let error = spool_event_for_later_replay(
        &forwarder.spool,
        "client-a",
        &event,
        GatewayForwardDropReason::GlobalQueueFull,
    )
    .await
    .unwrap_err()
    .to_string();

    assert!(error.contains("gateway spool disk cap exceeded"));
    assert_eq!(pending_spool_file_count(&dir), 1);
    let mut replay_index = GatewaySpoolReplayIndex::default();
    let replay = take_replay_items(&forwarder, &mut replay_index).await;
    assert_eq!(replay.len(), 1);
    if let GatewayForwardQueueItem::Spooled {
        path, disk_bytes, ..
    } = &replay[0].1
    {
        forwarder.spool.remove_spooled_file(path, *disk_bytes).await;
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn pending_spool_items_enter_the_replay_index_once_without_double_counting() {
    let dir = std::env::temp_dir().join(format!(
        "vpsman-gateway-spool-accounted-replay-{}",
        uuid::Uuid::new_v4()
    ));
    let forwarder = GatewayEventForwarder::with_spool_config(GatewaySpoolConfig::enabled(
        dir.clone(),
        1024 * 1024,
        8 * 1024 * 1024,
        30,
    ));
    let mut replay_index = GatewaySpoolReplayIndex::default();
    assert!(take_replay_items(&forwarder, &mut replay_index)
        .await
        .is_empty());
    let event = test_event(COMMAND_OUTPUT_PATH, br#"{"seq":1}"#);
    spool_event_for_later_replay(
        &forwarder.spool,
        "client-a",
        &event,
        GatewayForwardDropReason::TargetQueueFull,
    )
    .await
    .unwrap();
    let accounted_before = forwarder.spool.disk_bytes.load(Ordering::Relaxed);

    let first = take_replay_items(&forwarder, &mut replay_index).await;
    let after_first = forwarder.spool.disk_bytes.load(Ordering::Relaxed);
    let second = take_replay_items(&forwarder, &mut replay_index).await;
    let after_second = forwarder.spool.disk_bytes.load(Ordering::Relaxed);

    assert_eq!(first.len(), 1);
    assert!(second.is_empty());
    assert_eq!(after_first, accounted_before);
    assert_eq!(after_second, accounted_before);
    let third = take_replay_items(&forwarder, &mut replay_index).await;
    assert!(third.is_empty());
    if let GatewayForwardQueueItem::Spooled {
        path, disk_bytes, ..
    } = &first[0].1
    {
        forwarder.spool.remove_spooled_file(path, *disk_bytes).await;
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn pending_spool_items_preserve_target_and_order() {
    let dir = std::env::temp_dir().join(format!(
        "vpsman-gateway-spool-replay-{}",
        uuid::Uuid::new_v4()
    ));
    let forwarder = GatewayEventForwarder::with_spool_config(GatewaySpoolConfig::enabled(
        dir.clone(),
        1024 * 1024,
        8 * 1024 * 1024,
        30,
    ));
    let event = test_event("/internal/v1/gateway/command-output", br#"{"seq":1}"#);
    let candidate = forwarder
        .spool
        .spool_event("client-a", &event)
        .await
        .unwrap();

    let mut replay_index = GatewaySpoolReplayIndex::default();
    let replay = take_replay_items(&forwarder, &mut replay_index).await;

    assert_eq!(replay.len(), 1);
    assert_eq!(replay[0].0, "client-a");
    assert!(matches!(
        replay[0].1,
        GatewayForwardQueueItem::Spooled { .. }
    ));
    let _ = std::fs::remove_file(candidate.path);
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn pending_spool_items_replay_by_durable_enqueue_sequence() {
    let dir = std::env::temp_dir().join(format!(
        "vpsman-gateway-spool-replay-order-{}",
        uuid::Uuid::new_v4()
    ));
    let forwarder = GatewayEventForwarder::with_spool_config(GatewaySpoolConfig::enabled(
        dir.clone(),
        1024 * 1024,
        8 * 1024 * 1024,
        30,
    ));
    let mut later = test_event(COMMAND_OUTPUT_PATH, br#"{"seq":2}"#);
    later.created_unix = 100;
    later.enqueue_seq = 2;
    let mut earlier = test_event(COMMAND_OUTPUT_PATH, br#"{"seq":1}"#);
    earlier.created_unix = 100;
    earlier.enqueue_seq = 1;

    let later_candidate = forwarder
        .spool
        .spool_event("client-a", &later)
        .await
        .unwrap();
    let earlier_candidate = forwarder
        .spool
        .spool_event("client-a", &earlier)
        .await
        .unwrap();

    let mut replay_index = GatewaySpoolReplayIndex::default();
    let replay = take_replay_items(&forwarder, &mut replay_index).await;

    assert_eq!(replay.len(), 2);
    assert_eq!(replay[0].0, "client-a");
    assert_eq!(replay[1].0, "client-a");
    assert!(matches!(
        replay[0].1,
        GatewayForwardQueueItem::Spooled { enqueue_seq: 1, .. }
    ));
    assert!(matches!(
        replay[1].1,
        GatewayForwardQueueItem::Spooled { enqueue_seq: 2, .. }
    ));
    let _ = std::fs::remove_file(earlier_candidate.path);
    let _ = std::fs::remove_file(later_candidate.path);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn spool_header_requires_a_durable_enqueue_sequence() {
    let header = serde_json::from_value::<SpooledGatewayForwardHeader>(serde_json::json!({
        "schema_version": SPOOL_SCHEMA_VERSION,
        "api_url": "http://127.0.0.1:9",
        "path": COMMAND_OUTPUT_PATH,
        "internal_token": null,
        "kind": "command_output",
        "critical": true,
        "created_unix": 1,
        "body_sha256_hex": "ab".repeat(32)
    }));

    assert!(header.is_err());
}

#[tokio::test]
async fn forwarder_enqueue_sequence_starts_after_existing_spool() {
    let dir = std::env::temp_dir().join(format!(
        "vpsman-gateway-spool-seed-seq-{}",
        uuid::Uuid::new_v4()
    ));
    let config = GatewaySpoolConfig::enabled(dir.clone(), 1024 * 1024, 8 * 1024 * 1024, 30);
    let forwarder = GatewayEventForwarder::with_spool_config(config.clone());
    let mut existing = test_event(COMMAND_OUTPUT_PATH, br#"{"seq":1}"#);
    existing.enqueue_seq = u64::MAX - 4;
    let candidate = forwarder
        .spool
        .spool_event("client-a", &existing)
        .await
        .unwrap();

    let restarted = GatewayEventForwarder::with_spool_config(config);

    assert!(restarted.next_enqueue_seq() > existing.enqueue_seq);
    assert_eq!(
        restarted.spool.replay_header_reads.load(Ordering::Relaxed),
        1
    );
    let mut replay_index = GatewaySpoolReplayIndex::default();
    let replay = take_replay_items(&restarted, &mut replay_index).await;
    assert_eq!(replay.len(), 1);
    assert_eq!(replay[0].1.enqueue_seq(), existing.enqueue_seq);
    assert_eq!(
        restarted.spool.replay_header_reads.load(Ordering::Relaxed),
        1
    );
    assert_eq!(restarted.spool.replay_body_reads.load(Ordering::Relaxed), 0);
    let _ = std::fs::remove_file(candidate.path);
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn pending_spool_file_fences_later_critical_output_for_target() {
    let dir = std::env::temp_dir().join(format!(
        "vpsman-gateway-spool-fence-{}",
        uuid::Uuid::new_v4()
    ));
    let forwarder = GatewayEventForwarder::with_spool_config(GatewaySpoolConfig::enabled(
        dir.clone(),
        1024 * 1024,
        8 * 1024 * 1024,
        30,
    ));
    let first = test_event(COMMAND_OUTPUT_PATH, br#"{"seq":1}"#);
    let first_candidate = forwarder
        .spool
        .spool_event("client-a", &first)
        .await
        .unwrap();
    let second = test_event(COMMAND_OUTPUT_PATH, br#"{"seq":2}"#);

    forwarder
        .enqueue("client-a".to_string(), second, test_timeouts())
        .await
        .unwrap();

    let mut replay_index = GatewaySpoolReplayIndex::default();
    let replay = take_replay_items(&forwarder, &mut replay_index).await;
    assert_eq!(replay.len(), 2);
    assert_eq!(
        forwarder
            .metrics
            .current_queue_depth
            .load(Ordering::Relaxed),
        0
    );
    if let GatewayForwardQueueItem::Spooled {
        path, disk_bytes, ..
    } = &replay[0].1
    {
        forwarder.spool.remove_spooled_file(path, *disk_bytes).await;
    }
    if let GatewayForwardQueueItem::Spooled {
        path, disk_bytes, ..
    } = &replay[1].1
    {
        forwarder.spool.remove_spooled_file(path, *disk_bytes).await;
    }
    let _ = std::fs::remove_file(first_candidate.path);
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn pending_spool_items_quarantine_corrupt_entries() {
    let dir = std::env::temp_dir().join(format!(
        "vpsman-gateway-spool-corrupt-{}",
        uuid::Uuid::new_v4()
    ));
    let pending_dir = dir.join("pending");
    std::fs::create_dir_all(&pending_dir).unwrap();
    let target_hex = hex::encode("client-a".as_bytes());
    let corrupt_path = pending_dir.join(format!(
        "{}-{target_hex}-{}.spool",
        unix_now(),
        uuid::Uuid::new_v4()
    ));
    std::fs::write(&corrupt_path, b"not-a-valid-spool-file").unwrap();
    let malformed_path = pending_dir.join("malformed.spool");
    std::fs::write(&malformed_path, b"not-a-valid-spool-file").unwrap();
    let forwarder = GatewayEventForwarder::with_spool_config(GatewaySpoolConfig::enabled(
        dir.clone(),
        1024 * 1024,
        8 * 1024 * 1024,
        30,
    ));

    let mut replay_index = GatewaySpoolReplayIndex::default();
    let replay = take_replay_items(&forwarder, &mut replay_index).await;

    assert!(replay.is_empty());
    assert!(!corrupt_path.exists());
    assert!(!malformed_path.exists());
    let quarantined = dir
        .join("corrupt")
        .read_dir()
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(quarantined.len(), 2);
    assert_eq!(mode(&dir), 0o700);
    assert_eq!(mode(&dir.join("corrupt")), 0o700);
    assert!(quarantined.iter().all(|entry| mode(&entry.path()) == 0o600));
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn replay_quarantines_checksum_corruption_without_pressure_failure() {
    let dir = std::env::temp_dir().join(format!(
        "vpsman-gateway-spool-checksum-{}",
        uuid::Uuid::new_v4()
    ));
    let forwarder = GatewayEventForwarder::with_spool_config(GatewaySpoolConfig::enabled(
        dir.clone(),
        1024 * 1024,
        8 * 1024 * 1024,
        30,
    ));
    let candidate = forwarder
        .spool
        .spool_event(
            "client-a",
            &test_event(COMMAND_OUTPUT_PATH, br#"{"seq":1}"#),
        )
        .await
        .unwrap();
    let mut bytes = std::fs::read(&candidate.path).unwrap();
    *bytes.last_mut().unwrap() ^= 1;
    std::fs::write(&candidate.path, bytes).unwrap();
    let mut replay_index = GatewaySpoolReplayIndex::default();
    let mut replay = take_replay_items(&forwarder, &mut replay_index).await;

    assert!(queue_item_event(
        replay.pop().unwrap().1,
        "client-a",
        &forwarder.telemetry_pending,
        &forwarder.spool,
    )
    .await
    .is_none());
    assert!(!candidate.path.exists());
    assert_eq!(forwarder.spool.disk_bytes.load(Ordering::Relaxed), 0);
    assert!(!forwarder.spool.target_has_pending("client-a").await);
    assert_eq!(forwarder.metrics.snapshot().dropped_events, 0);
    assert_eq!(forwarder.metrics.snapshot().critical_failures, 0);
    assert_eq!(pending_spool_file_count(&dir), 0);
    assert_eq!(dir.join("corrupt").read_dir().unwrap().count(), 1);
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn failed_quarantine_preserves_accounting_and_retries_on_next_replay_wake() {
    let dir = std::env::temp_dir().join(format!(
        "vpsman-gateway-spool-quarantine-retry-{}",
        uuid::Uuid::new_v4()
    ));
    let forwarder = GatewayEventForwarder::with_spool_config(GatewaySpoolConfig::enabled(
        dir.clone(),
        1024 * 1024,
        8 * 1024 * 1024,
        30,
    ));
    let candidate = forwarder
        .spool
        .spool_event(
            "client-a",
            &test_event(COMMAND_OUTPUT_PATH, br#"{"seq":1}"#),
        )
        .await
        .unwrap();
    let mut bytes = std::fs::read(&candidate.path).unwrap();
    *bytes.last_mut().unwrap() ^= 1;
    std::fs::write(&candidate.path, bytes).unwrap();
    std::fs::write(dir.join("corrupt"), b"block directory creation").unwrap();
    let mut replay_index = GatewaySpoolReplayIndex::default();
    let mut replay = take_replay_items(&forwarder, &mut replay_index).await;
    assert!(queue_item_event(
        replay.pop().unwrap().1,
        "client-a",
        &forwarder.telemetry_pending,
        &forwarder.spool,
    )
    .await
    .is_none());
    assert!(candidate.path.exists());
    assert!(forwarder.spool.disk_bytes.load(Ordering::Relaxed) > 0);
    assert_eq!(forwarder.spool.quarantine_retries.lock().unwrap().len(), 1);

    std::fs::remove_file(dir.join("corrupt")).unwrap();
    forwarder
        .spool
        .refresh_replay_index(&mut replay_index)
        .await
        .unwrap();
    assert!(!candidate.path.exists());
    assert_eq!(forwarder.spool.disk_bytes.load(Ordering::Relaxed), 0);
    assert_eq!(dir.join("corrupt").read_dir().unwrap().count(), 1);
    let _ = std::fs::remove_dir_all(dir);
}

fn pending_spool_file_count(dir: &Path) -> usize {
    std::fs::read_dir(dir.join("pending"))
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|entry| {
                    entry
                        .path()
                        .extension()
                        .and_then(|extension| extension.to_str())
                        == Some("spool")
                })
                .count()
        })
        .unwrap_or(0)
}

#[test]
fn replay_scheduler_wakes_only_the_exact_blocked_target() {
    let mut index = GatewaySpoolReplayIndex::default();
    for target in 0..2_000 {
        index.insert(synthetic_spool_candidate(&format!("client-{target}"), 1));
    }
    while let Some(candidate) = index.pop_next() {
        let target = candidate.target_key.clone();
        index.block_target(&target);
        index.insert(candidate);
    }
    assert!(index.ready_targets.is_empty());
    index.unblock_target("client-1234");
    assert_eq!(index.ready_targets.len(), 1);
    assert_eq!(index.pop_next().unwrap().target_key, "client-1234");
    assert!(index.pop_next().is_none());
}

#[tokio::test]
async fn global_queue_slot_reservation_never_exceeds_the_exact_cap() {
    let metrics = Arc::new(GatewayForwardMetrics::default());
    let mut attempts = Vec::new();
    for _ in 0..GLOBAL_QUEUE_CAPACITY + 64 {
        let metrics = metrics.clone();
        attempts.push(tokio::spawn(
            async move { metrics.try_reserve_queue_slot() },
        ));
    }
    let mut admitted = 0;
    let mut first_slots = 0;
    for attempt in attempts {
        if let Some(previous) = attempt.await.unwrap() {
            admitted += 1;
            first_slots += u64::from(previous == 0);
        }
    }
    assert_eq!(admitted, GLOBAL_QUEUE_CAPACITY);
    assert_eq!(first_slots, 1);
    assert_eq!(
        metrics.current_queue_depth.load(Ordering::Relaxed),
        GLOBAL_QUEUE_CAPACITY
    );
}

#[test]
fn startup_removes_only_exact_writer_owned_temp_files() {
    use std::os::unix::fs::symlink;

    let dir = std::env::temp_dir().join(format!(
        "vpsman-gateway-spool-temp-cleanup-{}",
        uuid::Uuid::new_v4()
    ));
    let pending = dir.join("pending");
    std::fs::create_dir_all(&pending).unwrap();
    let regular = pending.join(format!(".{}.tmp", uuid::Uuid::new_v4()));
    let link = pending.join(format!(".{}.tmp", uuid::Uuid::new_v4()));
    let directory = pending.join(format!(".{}.tmp", uuid::Uuid::new_v4()));
    let lookalike = pending.join(".not-a-uuid.tmp");
    std::fs::write(&regular, b"partial").unwrap();
    symlink(&regular, &link).unwrap();
    std::fs::create_dir(&directory).unwrap();
    std::fs::write(&lookalike, b"keep").unwrap();

    let spool = GatewayForwardSpool::new(GatewaySpoolConfig::enabled(
        dir.clone(),
        1024 * 1024,
        8 * 1024 * 1024,
        30,
    ));
    let startup = spool.bootstrap_pending_candidates_sync();
    assert!(startup.fatal_error.is_none());
    assert!(!regular.exists());
    assert!(!link.exists());
    assert!(directory.is_dir());
    assert!(lookalike.exists());
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn short_durable_admission_lane_orders_same_target_and_not_other_targets() {
    let dir = std::env::temp_dir().join(format!(
        "vpsman-gateway-short-admission-{}",
        uuid::Uuid::new_v4()
    ));
    let forwarder = Arc::new(GatewayEventForwarder::with_spool_config(
        GatewaySpoolConfig::enabled(dir.clone(), 1024 * 1024, 16 * 1024 * 1024, 30),
    ));
    let lane = Arc::new(Mutex::new(()));
    forwarder
        .target_admission_lanes
        .lock()
        .await
        .insert("client-a".to_string(), lane.clone());
    let held = lane.lock().await;
    let mut first = Box::pin(forwarder.enqueue(
        "client-a".to_string(),
        test_event(COMMAND_OUTPUT_PATH, &vec![b'1'; 1024 * 1024 + 1]),
        test_timeouts(),
    ));
    std::future::poll_fn(|context| {
        assert!(first.as_mut().poll(context).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    let mut second = Box::pin(forwarder.enqueue(
        "client-a".to_string(),
        test_event(COMMAND_OUTPUT_PATH, &vec![b'2'; 1024 * 1024 + 1]),
        test_timeouts(),
    ));
    std::future::poll_fn(|context| {
        assert!(second.as_mut().poll(context).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    forwarder
        .enqueue(
            "client-b".to_string(),
            test_event(COMMAND_OUTPUT_PATH, &vec![b'x'; 1024 * 1024 + 1]),
            test_timeouts(),
        )
        .await
        .unwrap();
    drop(held);
    first.await.unwrap();
    second.await.unwrap();
    let mut index = GatewaySpoolReplayIndex::default();
    let replay = take_replay_items(&forwarder, &mut index).await;
    let mut client_a = Vec::new();
    for (target, item) in replay {
        if target == "client-a" {
            let GatewayForwardQueueItem::Spooled { path, .. } = item else {
                panic!("over-budget durable output is disk-backed");
            };
            client_a.push(forwarder.spool.load_spooled_event(&path).await.unwrap());
        }
    }
    assert_eq!(client_a.len(), 2);
    assert_eq!(client_a[0].body[0], b'1');
    assert_eq!(client_a[1].body[0], b'2');
    let _ = std::fs::remove_dir_all(dir);
}

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

fn test_timeouts() -> Arc<StdRwLock<GatewayHttpTimeouts>> {
    Arc::new(StdRwLock::new(GatewayHttpTimeouts::default()))
}
