use super::*;

#[tokio::test]
async fn terminal_registry_expires_disconnected_sessions_by_capped_idle_timeout() {
    let registry = TerminalRegistry {
        sessions: Mutex::new(HashMap::new()),
    };
    let long_idle_session = uuid::Uuid::new_v4();
    registry
        .insert(
            long_idle_session,
            test_registry_entry(long_idle_session, 86_400).await,
        )
        .await;
    registry.mark_disconnected().await;
    set_disconnected_since(
        &registry,
        long_idle_session,
        unix_now().saturating_sub(3_599),
    )
    .await;
    assert!(registry.disconnected_expired_sessions().await.is_empty());

    registry.mark_connected().await;
    assert_disconnected_since(&registry, long_idle_session, None).await;
    registry.mark_disconnected().await;
    set_disconnected_since(
        &registry,
        long_idle_session,
        unix_now().saturating_sub(3_600),
    )
    .await;
    let expired = registry.disconnected_expired_sessions().await;
    assert_eq!(expired.len(), 1);
    assert_eq!(expired[0].idle_timeout_secs, 86_400);
    assert_eq!(registry.session_count().await, 0);

    let short_idle_session = uuid::Uuid::new_v4();
    registry
        .insert(
            short_idle_session,
            test_registry_entry(short_idle_session, 30).await,
        )
        .await;
    registry.mark_disconnected().await;
    set_disconnected_since(&registry, short_idle_session, unix_now().saturating_sub(29)).await;
    assert!(registry.disconnected_expired_sessions().await.is_empty());
    set_disconnected_since(&registry, short_idle_session, unix_now().saturating_sub(30)).await;
    assert_eq!(registry.disconnected_expired_sessions().await.len(), 1);
}

#[tokio::test]
async fn reliable_terminal_final_status_buffers_when_stream_receiver_closed() {
    drain_pending_terminal_final_events().await;
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

    let pending = drain_pending_terminal_final_events().await;
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].session_id, session_id);
    assert!(pending[0].output.done);
    let status: serde_json::Value = serde_json::from_slice(&pending[0].output.data).unwrap();
    assert_eq!(status["status"], "disconnected_timeout");
    assert_eq!(status["reason"], "gateway_disconnected_timeout");
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
    }
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
