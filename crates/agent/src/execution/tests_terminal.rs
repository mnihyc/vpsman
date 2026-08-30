use super::*;

#[tokio::test]
async fn terminal_registry_expires_disconnected_sessions_by_capped_idle_timeout() {
    let registry = TerminalRegistry {
        sessions: Mutex::new(HashMap::new()),
    };
    let long_idle_session = uuid::Uuid::new_v4();
    let long_idle_entry = test_registry_entry(long_idle_session, 86_400).await;
    let long_idle_job = long_idle_entry.handle.open_job_id;
    registry.insert(long_idle_session, long_idle_entry).await;
    registry.mark_disconnected().await;
    set_disconnected_since(
        &registry,
        long_idle_session,
        unix_now().saturating_sub(3_599),
    )
    .await;
    assert!(
        !registry
            .exact_session_disconnected_expired(long_idle_session, long_idle_job)
            .await
    );

    registry.mark_connected().await;
    assert_disconnected_since(&registry, long_idle_session, None).await;
    registry.mark_disconnected().await;
    set_disconnected_since(
        &registry,
        long_idle_session,
        unix_now().saturating_sub(3_600),
    )
    .await;
    assert!(
        registry
            .exact_session_disconnected_expired(long_idle_session, long_idle_job)
            .await
    );
    let expired = registry
        .remove_if_current(long_idle_session, long_idle_job)
        .await
        .unwrap();
    assert_eq!(expired.idle_timeout_secs, 86_400);
    assert_eq!(registry.session_count().await, 0);

    let short_idle_session = uuid::Uuid::new_v4();
    let short_idle_entry = test_registry_entry(short_idle_session, 30).await;
    let short_idle_job = short_idle_entry.handle.open_job_id;
    registry.insert(short_idle_session, short_idle_entry).await;
    registry.mark_disconnected().await;
    set_disconnected_since(&registry, short_idle_session, unix_now().saturating_sub(29)).await;
    assert!(
        !registry
            .exact_session_disconnected_expired(short_idle_session, short_idle_job)
            .await
    );
    set_disconnected_since(&registry, short_idle_session, unix_now().saturating_sub(30)).await;
    assert!(
        registry
            .exact_session_disconnected_expired(short_idle_session, short_idle_job)
            .await
    );
}

#[tokio::test]
async fn stale_terminal_consumer_cannot_remove_a_replacement_session() {
    let registry = TerminalRegistry {
        sessions: Mutex::new(HashMap::new()),
    };
    let session_id = uuid::Uuid::new_v4();
    let first = test_registry_entry(session_id, 30).await;
    let first_job_id = first.handle.open_job_id;
    registry.insert(session_id, first).await;
    let replacement = test_registry_entry(session_id, 30).await;
    let replacement_job_id = replacement.handle.open_job_id;
    registry.insert(session_id, replacement).await;

    assert!(registry
        .remove_if_current(session_id, first_job_id)
        .await
        .is_none());
    assert_eq!(
        registry
            .get_handle(session_id)
            .await
            .map(|handle| handle.open_job_id),
        Some(replacement_job_id)
    );
}

#[tokio::test]
async fn reliable_terminal_final_status_buffers_when_stream_receiver_closed() {
    let _test_guard = pending_terminal_test_lock().lock().await;
    clear_pending_terminal_final_events().await;
    let session_id = uuid::Uuid::new_v4();
    let entry = test_registry_entry(session_id, 30).await;
    let (stream_tx, stream_rx) = mpsc::channel(1);
    drop(stream_rx);
    *entry.handle.stream_tx.lock().await = Some(stream_tx);

    entry
        .handle
        .emit_stream_status_with_reason(
            "terminal_stream",
            "disconnected_timeout",
            true,
            Some(0),
            Some("gateway_disconnected_timeout"),
        )
        .await;

    let pending = pending_terminal_final_events().await;
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].event.session_id, session_id);
    assert!(pending[0].event.output.done);
    let status: serde_json::Value = serde_json::from_slice(&pending[0].event.output.data).unwrap();
    assert_eq!(status["status"], "disconnected_timeout");
    assert_eq!(status["reason"], "gateway_disconnected_timeout");
    acknowledge_pending_terminal_final_event(&pending[0]).await;
}

#[tokio::test]
async fn terminal_final_handoff_acknowledges_only_the_exact_session_generation() {
    let _test_guard = pending_terminal_test_lock().lock().await;
    clear_pending_terminal_final_events().await;
    let session_id = uuid::Uuid::new_v4();
    let entry = test_registry_entry(session_id, 30).await;
    let (stream_tx, stream_rx) = mpsc::channel(1);
    drop(stream_rx);
    *entry.handle.stream_tx.lock().await = Some(stream_tx);

    entry
        .handle
        .emit_stream_status("terminal_stream", "exited", true, Some(0))
        .await;
    let first = pending_terminal_final_events().await;
    assert_eq!(first.len(), 1);

    entry
        .handle
        .emit_stream_status_with_reason("terminal_close", "closed", true, Some(0), Some("operator"))
        .await;
    acknowledge_pending_terminal_final_event(&first[0]).await;

    let current = pending_terminal_final_events().await;
    assert_eq!(current.len(), 1);
    let status: serde_json::Value = serde_json::from_slice(&current[0].event.output.data).unwrap();
    assert_eq!(status["status"], "closed");
    acknowledge_pending_terminal_final_event(&current[0]).await;
}

#[tokio::test]
async fn retained_terminal_final_wakes_the_connected_exact_handoff_consumer() {
    let _test_guard = pending_terminal_test_lock().lock().await;
    clear_pending_terminal_final_events().await;
    let session_id = uuid::Uuid::new_v4();
    let entry = test_registry_entry(session_id, 30).await;
    let (stream_tx, stream_rx) = mpsc::channel(1);
    drop(stream_rx);
    *entry.handle.stream_tx.lock().await = Some(stream_tx);
    let ready = pending_terminal_final_event_ready();
    tokio::pin!(ready);
    entry
        .handle
        .emit_stream_status("terminal_stream", "exited", true, Some(0))
        .await;

    time::timeout(Duration::from_secs(1), &mut ready)
        .await
        .expect("retained final must wake the live connected consumer");
    clear_pending_terminal_final_events().await;
}

#[tokio::test]
async fn terminal_open_owner_serializes_only_the_exact_session() {
    let session_id = uuid::Uuid::new_v4();
    let other_session_id = uuid::Uuid::new_v4();
    let first = acquire_terminal_open_owner(session_id).await;
    let same_session = tokio::spawn(acquire_terminal_open_owner(session_id));

    let other = time::timeout(
        Duration::from_millis(100),
        acquire_terminal_open_owner(other_session_id),
    )
    .await
    .expect("an unrelated terminal session must have an independent owner");
    drop(other);
    assert!(!same_session.is_finished());

    drop(first);
    let next = time::timeout(Duration::from_secs(1), same_session)
        .await
        .expect("the same terminal session must proceed after its owner releases")
        .expect("terminal owner task must complete");
    drop(next);
}

#[test]
fn terminal_output_buffer_retains_tail_and_reports_truncation() {
    let mut output = TerminalOutputBuffer {
        chunks: VecDeque::new(),
        next_seq: 1,
        retained_bytes: 0,
        max_retained_bytes: 8,
        dropped_bytes: 0,
        dropped_chunks: 0,
    };
    output.push(b"abc".to_vec());
    output.push(b"def".to_vec());
    output.push(b"ghijklmnop".to_vec());

    let snapshot = output.snapshot_from(1);

    assert_eq!(snapshot.chunks.len(), 1);
    assert_eq!(snapshot.chunks[0].seq, 3);
    assert_eq!(snapshot.chunks[0].data, b"ijklmnop");
    assert_eq!(snapshot.range.first_seq, Some(3));
    assert_eq!(snapshot.range.next_seq, 4);
    assert_eq!(snapshot.range.retained_first_seq, Some(3));
    assert_eq!(snapshot.range.retained_bytes, 8);
    assert_eq!(snapshot.range.dropped_bytes, 8);
    assert_eq!(snapshot.range.dropped_chunks, 2);
    assert!(snapshot.range.replay_truncated);

    let current_snapshot = output.snapshot_from(3);
    assert!(!current_snapshot.range.replay_truncated);
}

async fn test_registry_entry(
    session_id: uuid::Uuid,
    idle_timeout_secs: u32,
) -> TerminalRegistryEntry {
    TerminalRegistryEntry {
        handle: TerminalSessionHandle {
            session_id,
            open_job_id: uuid::Uuid::new_v4(),
            writer: Arc::new(Mutex::new(
                tokio::fs::OpenOptions::new()
                    .write(true)
                    .open("/dev/null")
                    .await
                    .unwrap(),
            )),
            output: Arc::new(Mutex::new(TerminalOutputBuffer::new(1024))),
            exit_code: Arc::new(Mutex::new(None)),
            process_group_id: 0,
            last_activity: Arc::new(AtomicU64::new(unix_now())),
            stream_tx: Arc::new(Mutex::new(None)),
        },
        last_delivered_seq: 1,
        last_input_seq: 0,
        disconnected_since: None,
        idle_timeout_secs,
        cols: 120,
        rows: 40,
        _capacity_owner: None,
        _session_owner: None,
    }
}

async fn clear_pending_terminal_final_events() {
    for pending in pending_terminal_final_events().await {
        acknowledge_pending_terminal_final_event(&pending).await;
    }
}

fn pending_terminal_test_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

async fn set_disconnected_since(
    registry: &TerminalRegistry,
    session_id: uuid::Uuid,
    disconnected_since: u64,
) {
    registry
        .sessions
        .lock()
        .await
        .get_mut(&session_id)
        .unwrap()
        .disconnected_since = Some(disconnected_since);
}

async fn assert_disconnected_since(
    registry: &TerminalRegistry,
    session_id: uuid::Uuid,
    expected: Option<u64>,
) {
    assert_eq!(
        registry
            .sessions
            .lock()
            .await
            .get(&session_id)
            .unwrap()
            .disconnected_since,
        expected
    );
}
