use super::*;
use std::os::unix::fs::PermissionsExt;
use tokio::sync::{oneshot, watch};

static TEST_ENQUEUE_SEQ: AtomicU64 = AtomicU64::new(1);

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

async fn forward_once(
    event: &GatewayForwardEvent,
    session_rejection_handler: &StdRwLock<Option<GatewaySessionRejectionHandler>>,
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
        &spool,
        &runtime_config,
        &timeouts,
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
            recorded
                .lock()
                .unwrap()
                .push((client_id, rejected_session_id));
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
async fn successful_or_unrelated_responses_do_not_reject_the_session_fence() {
    let rejections = Arc::new(StdMutex::new(Vec::new()));
    let recorded = rejections.clone();
    let handler: GatewaySessionRejectionHandler =
        Arc::new(move |client_id, rejected_session_id| {
            recorded
                .lock()
                .unwrap()
                .push((client_id, rejected_session_id));
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
async fn shutdown_defers_non_command_events_to_spool() {
    let event = test_event("/internal/v1/gateway/agent-hello", br#"{}"#);
    let metrics = GatewayForwardMetrics::default();
    let critical_failure_handler = StdRwLock::new(None);
    let session_rejection_handler = StdRwLock::new(None);
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
        &spool,
        &runtime_config,
        &timeouts,
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
        &spool,
        &runtime_config,
        &timeouts,
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
async fn telemetry_admission_is_bounded_and_observable() {
    let forwarder = Arc::new(GatewayEventForwarder::with_config(
        GatewaySpoolConfig::disabled(),
        GatewayForwardConfig::default(),
        2,
    ));
    let (release_tx, release_rx) = watch::channel(false);
    let mut tasks = Vec::new();
    for _ in 0..6 {
        let forwarder = forwarder.clone();
        let mut release_rx = release_rx.clone();
        tasks.push(tokio::spawn(async move {
            let _permit = acquire_telemetry_admission(
                forwarder.telemetry_admission.clone(),
                forwarder.metrics.clone(),
            )
            .await
            .expect("admission remains open");
            if !*release_rx.borrow() {
                release_rx.changed().await.unwrap();
            }
        }));
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
    let snapshot = forwarder.metrics.snapshot();
    assert_eq!(snapshot.telemetry_admission_limit, 2);
    assert_eq!(snapshot.telemetry_admission_active, 2);
    assert_eq!(snapshot.telemetry_admission_waiting, 4);

    release_tx.send(true).unwrap();
    for task in tasks {
        task.await.unwrap();
    }
    let snapshot = forwarder.metrics.snapshot();
    assert_eq!(snapshot.telemetry_admission_active, 0);
    assert_eq!(snapshot.telemetry_admission_waiting, 0);
}

#[tokio::test]
async fn telemetry_waits_for_admission_without_blocking_same_client_lifecycle() {
    let forwarder = GatewayEventForwarder::with_config(
        GatewaySpoolConfig::disabled(),
        GatewayForwardConfig::default(),
        1,
    );
    let held_permit = forwarder
        .telemetry_admission
        .clone()
        .acquire_owned()
        .await
        .unwrap();
    let mut telemetry = test_event("/internal/v1/gateway/telemetry", br#"{"seq":1}"#);
    telemetry.created_at = time::Instant::now() - TELEMETRY_EVENT_TTL;
    forwarder
        .enqueue("client-a".to_string(), telemetry, test_timeouts())
        .await
        .unwrap();

    time::timeout(Duration::from_secs(1), async {
        while forwarder.metrics.snapshot().telemetry_admission_waiting != 1 {
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

    drop(held_permit);
    time::timeout(Duration::from_secs(1), async {
        while forwarder.metrics.snapshot().current_queue_depth != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let pending = forwarder.telemetry_pending.lock().await;
    assert!(pending.events.is_empty());
    assert!(pending.draining_targets.is_empty());
}

#[tokio::test]
async fn idle_forward_queue_is_reaped_on_next_enqueue() {
    let forwarder = GatewayEventForwarder::default();
    let (sender, _receiver) = mpsc::channel(PER_TARGET_QUEUE_CAPACITY);
    forwarder.queues.lock().await.insert(
        "idle-client".to_string(),
        GatewayForwardQueue {
            sender,
            last_enqueue_unix: unix_now().saturating_sub(QUEUE_IDLE_REAP_SECS + 1),
        },
    );

    forwarder
        .enqueue(
            "active-client".to_string(),
            test_event("/internal/v1/gateway/agent-hello", br#"{}"#),
            test_timeouts(),
        )
        .await
        .unwrap();

    let queues = forwarder.queues.lock().await;
    assert!(!queues.contains_key("idle-client"));
    assert!(queues.contains_key("active-client"));
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
            if let Some(sender) = sent.lock().unwrap().take() {
                let _ = sender.send((client_id, reason));
            }
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

    let GatewayForwardQueueItem::Spooled {
        path, disk_bytes, ..
    } = item
    else {
        panic!("command output above RAM budget should spool");
    };
    assert!(path.exists());
    assert!(disk_bytes > body.len() as u64);
    assert_eq!(mode(&dir), 0o700);
    assert_eq!(mode(&dir.join("pending")), 0o700);
    assert_eq!(mode(&path), 0o600);
    let decoded = forwarder.spool.load_spooled_event(&path).await.unwrap();
    assert_eq!(decoded.body, body);
    assert_eq!(decoded.kind, GatewayForwardEventKind::CommandOutput);
    assert!(decoded.critical);
    forwarder.spool.remove_spooled_file(&path, disk_bytes).await;
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

    let GatewayForwardQueueItem::Spooled {
        path, disk_bytes, ..
    } = item
    else {
        panic!("terminal output above RAM budget should spool");
    };
    let decoded = forwarder.spool.load_spooled_event(&path).await.unwrap();
    assert_eq!(decoded.kind, GatewayForwardEventKind::TerminalOutput);
    assert!(!decoded.critical);
    forwarder.spool.remove_spooled_file(&path, disk_bytes).await;
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

    let GatewayForwardQueueItem::Spooled {
        path, disk_bytes, ..
    } = forwarder
        .spool
        .spool_event("client-a", &event)
        .await
        .unwrap()
    else {
        panic!("spool_event must return a spooled item");
    };

    let header = forwarder.spool.load_spooled_header(&path).await.unwrap();
    assert_eq!(header.command_output, Some(replay_key));
    assert!(header.critical);
    forwarder.spool.remove_spooled_file(&path, disk_bytes).await;
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
        })
        .unwrap();
    forwarder.queues.lock().await.insert(
        "client-a".to_string(),
        GatewayForwardQueue {
            sender,
            last_enqueue_unix: unix_now(),
        },
    );
    let event = test_event(COMMAND_OUTPUT_PATH, br#"{"seq":1}"#);
    let item = forwarder
        .spool
        .spool_event("client-a", &event)
        .await
        .unwrap();
    let GatewayForwardQueueItem::Spooled {
        path, disk_bytes, ..
    } = &item
    else {
        panic!("spool_event must return a spooled item");
    };
    let path = path.clone();
    let disk_bytes = *disk_bytes;
    let accounted_before = forwarder.spool.disk_bytes.load(Ordering::Relaxed);

    let result = forwarder
        .enqueue_queue_item("client-a".to_string(), item, test_timeouts())
        .await;

    assert!(result.is_err());
    assert!(path.exists());
    assert_eq!(
        forwarder.spool.disk_bytes.load(Ordering::Relaxed),
        accounted_before
    );
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
    let replay = forwarder.spool.pending_items().await;
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
async fn pending_spool_items_do_not_double_count_accounted_file() {
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

    let first = forwarder.spool.pending_items().await;
    let after_first = forwarder.spool.disk_bytes.load(Ordering::Relaxed);
    let second = forwarder.spool.pending_items().await;
    let after_second = forwarder.spool.disk_bytes.load(Ordering::Relaxed);

    assert_eq!(first.len(), 1);
    assert_eq!(second.len(), 1);
    assert_eq!(after_first, accounted_before);
    assert_eq!(after_second, accounted_before);
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
    let GatewayForwardQueueItem::Spooled { path, .. } = forwarder
        .spool
        .spool_event("client-a", &event)
        .await
        .unwrap()
    else {
        panic!("spool_event must return a spooled item");
    };

    let replay = forwarder.spool.pending_items().await;

    assert_eq!(replay.len(), 1);
    assert_eq!(replay[0].0, "client-a");
    assert!(matches!(
        replay[0].1,
        GatewayForwardQueueItem::Spooled {
            kind: GatewayForwardEventKind::CommandOutput,
            ..
        }
    ));
    let _ = std::fs::remove_file(path);
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

    let GatewayForwardQueueItem::Spooled {
        path: later_path, ..
    } = forwarder
        .spool
        .spool_event("client-a", &later)
        .await
        .unwrap()
    else {
        panic!("spool_event must return a spooled item");
    };
    let GatewayForwardQueueItem::Spooled {
        path: earlier_path, ..
    } = forwarder
        .spool
        .spool_event("client-a", &earlier)
        .await
        .unwrap()
    else {
        panic!("spool_event must return a spooled item");
    };

    let replay = forwarder.spool.pending_items().await;

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
    let _ = std::fs::remove_file(earlier_path);
    let _ = std::fs::remove_file(later_path);
    let _ = std::fs::remove_dir_all(dir);
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
    let GatewayForwardQueueItem::Spooled { path, .. } = forwarder
        .spool
        .spool_event("client-a", &existing)
        .await
        .unwrap()
    else {
        panic!("spool_event must return a spooled item");
    };

    let restarted = GatewayEventForwarder::with_spool_config(config);

    assert!(restarted.next_enqueue_seq() > existing.enqueue_seq);
    let _ = std::fs::remove_file(path);
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
    let GatewayForwardQueueItem::Spooled {
        path: first_path, ..
    } = forwarder
        .spool
        .spool_event("client-a", &first)
        .await
        .unwrap()
    else {
        panic!("spool_event must return a spooled item");
    };
    let second = test_event(COMMAND_OUTPUT_PATH, br#"{"seq":2}"#);

    forwarder
        .enqueue("client-a".to_string(), second, test_timeouts())
        .await
        .unwrap();

    let replay = forwarder.spool.pending_items().await;
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
    let _ = std::fs::remove_file(first_path);
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn pending_spool_items_quarantine_corrupt_entries() {
    let dir = std::env::temp_dir().join(format!(
        "vpsman-gateway-spool-corrupt-{}",
        uuid::Uuid::new_v4()
    ));
    let forwarder = GatewayEventForwarder::with_spool_config(GatewaySpoolConfig::enabled(
        dir.clone(),
        1024 * 1024,
        8 * 1024 * 1024,
        30,
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

    let replay = forwarder.spool.pending_items().await;

    assert!(replay.is_empty());
    assert!(!corrupt_path.exists());
    let quarantined_path = dir
        .join("corrupt")
        .read_dir()
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert_eq!(mode(&dir), 0o700);
    assert_eq!(mode(&dir.join("corrupt")), 0o700);
    assert_eq!(mode(&quarantined_path), 0o600);
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

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

fn test_timeouts() -> Arc<StdRwLock<GatewayHttpTimeouts>> {
    Arc::new(StdRwLock::new(GatewayHttpTimeouts::default()))
}
