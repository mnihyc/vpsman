use super::{
    build_terminal_replay_from_chunks, build_terminal_sessions, parse_terminal_event,
    upsert_memory_terminal_session, TerminalStatusOutput,
};
use crate::{
    auth_model::{AuthContext, OperatorPreferences, OperatorView},
    model::{JobHistoryView, JobTargetView},
    model_terminal::{TerminalOutputChunkRecord, TerminalSessionView},
    repository::{MemoryState, Repository},
};
use uuid::Uuid;
use vpsman_common::{
    CommandOutput, OutputStream, TerminalControlAck, TerminalControlAction, TerminalStreamOutput,
};

#[test]
fn builds_latest_open_terminal_session_with_start_metadata() {
    let session_id = Uuid::new_v4();
    let open_job = Uuid::new_v4();
    let outputs = vec![
        status_output(
            open_job,
            "edge-a",
            0,
            "400",
            serde_json::json!({
                "type": "terminal_stream",
                "status": "streaming",
                "session_id": session_id,
                "output_first_seq": 3,
                "output_next_seq": 5,
                "output_retained_first_seq": 1,
                "output_retained_bytes": 192,
                "output_dropped_bytes": 64,
                "output_dropped_chunks": 1,
                "output_replay_truncated": true,
                "session_exited": false
            }),
        ),
        status_output(
            open_job,
            "edge-a",
            0,
            "300",
            serde_json::json!({
                "type": "terminal_resize",
                "status": "resized",
                "session_id": session_id,
                "cols": 100,
                "rows": 30,
                "session_exited": false
            }),
        ),
        status_output(
            open_job,
            "edge-a",
            0,
            "200",
            serde_json::json!({
                "type": "terminal_input",
                "status": "accepted",
                "session_id": session_id,
                "input_seq": 7,
                "written_bytes": 3,
                "output_first_seq": 1,
                "output_next_seq": 3,
                "output_retained_first_seq": 1,
                "output_retained_bytes": 128,
                "output_dropped_bytes": 64,
                "output_dropped_chunks": 1,
                "output_replay_truncated": true,
                "session_exited": false
            }),
        ),
        status_output(
            open_job,
            "edge-a",
            0,
            "100",
            serde_json::json!({
                "type": "terminal_open",
                "status": "opened",
                "session_id": session_id,
                "argv": ["/bin/sh", "-l"],
                "cwd": "/root",
                "cols": 80,
                "rows": 24,
                "idle_timeout_secs": 600,
                "flow_window_bytes": 65536,
                "output_first_seq": 1,
                "output_next_seq": 1,
                "session_exited": false
            }),
        ),
    ];

    let sessions = build_terminal_sessions(outputs, 20, None).unwrap();

    assert_eq!(sessions.len(), 1);
    let session = &sessions[0];
    assert_eq!(session.session_id, session_id);
    assert_eq!(session.client_id, "edge-a");
    assert_eq!(session.state, "open");
    assert_eq!(session.last_status, "streaming");
    assert_eq!(session.job_id, open_job);
    assert_eq!(session.argv, vec!["/bin/sh".to_string(), "-l".to_string()]);
    assert_eq!(session.cwd.as_deref(), Some("/root"));
    assert_eq!(session.cols, Some(100));
    assert_eq!(session.rows, Some(30));
    assert_eq!(session.idle_timeout_secs, Some(600));
    assert_eq!(session.flow_window_bytes, Some(65536));
    assert_eq!(session.output_next_seq, Some(5));
    assert_eq!(session.output_retained_first_seq, Some(1));
    assert_eq!(session.output_retained_bytes, Some(192));
    assert_eq!(session.output_dropped_bytes, Some(64));
    assert_eq!(session.output_dropped_chunks, Some(1));
    assert!(session.output_replay_truncated);
    assert_eq!(session.last_input_seq, 7);
    assert_eq!(session.opened_at.as_deref(), Some("100"));
}

#[test]
fn filters_terminal_sessions_and_marks_closed() {
    let wanted = Uuid::new_v4();
    let other = Uuid::new_v4();
    let wanted_job = Uuid::new_v4();
    let outputs = vec![
        status_output(
            wanted_job,
            "edge-b",
            0,
            "300",
            serde_json::json!({
                "type": "terminal_close",
                "status": "closed",
                "session_id": wanted,
                "reason": "operator",
                "output_first_seq": 4,
                "output_next_seq": 5,
                "output_retained_first_seq": 4,
                "output_retained_bytes": 512,
                "output_dropped_bytes": 0,
                "output_dropped_chunks": 0,
                "output_replay_truncated": false,
                "session_exited": true
            }),
        ),
        status_output(
            wanted_job,
            "edge-b",
            0,
            "100",
            serde_json::json!({
                "type": "terminal_open",
                "status": "opened",
                "session_id": wanted,
                "argv": ["/bin/bash"],
                "cols": 120,
                "rows": 40,
                "idle_timeout_secs": 300,
                "flow_window_bytes": 32768,
                "session_exited": false
            }),
        ),
        status_output(
            Uuid::new_v4(),
            "edge-c",
            0,
            "200",
            serde_json::json!({
                "type": "terminal_open",
                "status": "opened",
                "session_id": other,
                "argv": ["/bin/sh"],
                "session_exited": false
            }),
        ),
    ];

    let sessions = build_terminal_sessions(outputs, 20, Some(wanted)).unwrap();

    assert_eq!(sessions.len(), 1);
    let session = &sessions[0];
    assert_eq!(session.state, "closed");
    assert_eq!(session.close_reason.as_deref(), Some("operator"));
    assert_eq!(session.argv, vec!["/bin/bash".to_string()]);
    assert_eq!(session.cols, Some(120));
    assert_eq!(session.rows, Some(40));
    assert_eq!(session.output_retained_bytes, Some(512));
    assert_eq!(session.output_dropped_bytes, Some(0));
    assert!(!session.output_replay_truncated);
    assert_eq!(session.job_id, wanted_job);
}

#[test]
fn delayed_nonterminal_output_cannot_reopen_aggregated_closed_session() {
    let session_id = Uuid::new_v4();
    let open_job = Uuid::new_v4();
    let outputs = vec![
        status_output(
            open_job,
            "edge-a",
            0,
            "200",
            serde_json::json!({
                "type": "terminal_stream",
                "status": "streaming",
                "session_id": session_id,
                "input_seq": 2,
                "output_next_seq": 8,
                "output_retained_first_seq": 2,
                "output_retained_bytes": 128,
                "output_dropped_bytes": 10,
                "output_dropped_chunks": 1,
                "session_exited": false
            }),
        ),
        status_output(
            open_job,
            "edge-a",
            0,
            "100",
            serde_json::json!({
                "type": "terminal_close",
                "status": "closed",
                "session_id": session_id,
                "output_next_seq": 10,
                "output_retained_first_seq": 5,
                "output_retained_bytes": 512,
                "output_dropped_bytes": 100,
                "output_dropped_chunks": 4,
                "session_exited": false
            }),
        ),
    ];

    let sessions = build_terminal_sessions(outputs, 20, None).unwrap();

    assert_eq!(sessions.len(), 1);
    let session = &sessions[0];
    assert_eq!(session.state, "closed");
    assert_eq!(session.last_event, "terminal_close");
    assert_eq!(session.job_id, open_job);
    assert_eq!(session.observed_at, "100");
    assert_eq!(session.output_next_seq, Some(10));
    assert_eq!(session.output_retained_first_seq, Some(5));
    assert_eq!(session.output_retained_bytes, Some(512));
    assert_eq!(session.output_dropped_bytes, Some(100));
    assert_eq!(session.output_dropped_chunks, Some(4));
    assert_eq!(session.last_input_seq, 2);
}

#[test]
fn delayed_terminal_event_cannot_reopen_or_regress_session_counters() {
    let session_id = Uuid::new_v4();
    let terminal_job_id = Uuid::new_v4();
    let mut existing = test_terminal_session("edge-a", session_id, terminal_job_id, 7, "closed");
    existing.output_next_seq = Some(10);
    existing.output_retained_first_seq = Some(5);
    existing.output_retained_bytes = Some(512);
    existing.output_dropped_bytes = Some(100);
    existing.output_dropped_chunks = Some(4);
    existing.last_event = "terminal_close".to_string();
    existing.observed_at = "2026-06-21T00:00:00Z".to_string();
    let mut sessions = vec![existing];
    let delayed = parse_terminal_event(status_output(
        terminal_job_id,
        "edge-a",
        0,
        "2026-06-22T00:00:00Z",
        serde_json::json!({
            "type": "terminal_stream",
            "status": "streaming",
            "session_id": session_id,
            "input_seq": 2,
            "output_next_seq": 8,
            "output_retained_first_seq": 2,
            "output_retained_bytes": 128,
            "output_dropped_bytes": 10,
            "output_dropped_chunks": 1,
            "session_exited": false
        }),
    ))
    .unwrap();

    upsert_memory_terminal_session(
        &mut sessions,
        super::TerminalAggregate::new(delayed).into_view(),
    )
    .unwrap();

    let session = &sessions[0];
    assert_eq!(session.state, "closed");
    assert_eq!(session.last_event, "terminal_close");
    assert_eq!(session.job_id, terminal_job_id);
    assert_eq!(session.output_next_seq, Some(10));
    assert_eq!(session.output_retained_first_seq, Some(5));
    assert_eq!(session.output_retained_bytes, Some(512));
    assert_eq!(session.output_dropped_bytes, Some(100));
    assert_eq!(session.output_dropped_chunks, Some(4));
    assert_eq!(session.last_input_seq, 7);
}

#[test]
fn terminal_source_order_uses_full_precision_and_instant_equivalence() {
    let session_id = Uuid::new_v4();
    let job_id = Uuid::new_v4();
    let mut existing = test_terminal_session("edge-a", session_id, job_id, 0, "open");
    existing.observed_at = "2026-06-21T00:00:00Z".to_string();
    let mut fractional = test_terminal_session("edge-a", session_id, job_id, 0, "open");
    fractional.observed_at = "2026-06-21T00:00:00.1Z".to_string();

    assert!(super::terminal_source_is_at_least_as_new(&existing, &fractional).unwrap());

    let mut equivalent = test_terminal_session("edge-a", session_id, job_id, 0, "open");
    equivalent.observed_at = "2026-06-21T01:00:00+01:00".to_string();
    assert!(super::terminal_source_is_at_least_as_new(&existing, &equivalent).unwrap());

    equivalent.observed_at = "not-a-timestamp".to_string();
    assert!(super::terminal_source_is_at_least_as_new(&existing, &equivalent).is_err());
}

#[test]
fn builds_durable_terminal_replay_from_persisted_pty_outputs() {
    let session_id = Uuid::new_v4();
    let input_job = Uuid::new_v4();
    let poll_job = Uuid::new_v4();
    let outputs = vec![
        replay_chunk(input_job, "edge-a", session_id, 1, "100", b"one\n"),
        replay_chunk(input_job, "edge-a", session_id, 2, "100", b"two\n"),
        replay_chunk(poll_job, "edge-a", session_id, 3, "200", b"three\n"),
    ];

    let replay =
        build_terminal_replay_from_chunks("edge-a", session_id, outputs, 2, 10, 1000, true, 4);

    assert_eq!(replay.client_id, "edge-a");
    assert_eq!(replay.session_id, session_id);
    assert_eq!(replay.from_seq, 2);
    assert_eq!(replay.available_first_seq, Some(2));
    assert_eq!(replay.next_seq, 4);
    assert_eq!(replay.chunk_count, 2);
    assert_eq!(replay.byte_count, 10);
    assert!(!replay.truncated);
    assert_eq!(replay.chunks[0].terminal_seq, 2);
    assert_eq!(replay.chunks[0].job_id, input_job);
    assert_eq!(replay.chunks[0].data_base64.as_deref(), Some("dHdvCg=="));
    assert_eq!(replay.chunks[1].terminal_seq, 3);
    assert_eq!(replay.chunks[1].job_id, poll_job);
    assert_eq!(replay.chunks[1].data_base64.as_deref(), Some("dGhyZWUK"));
}

#[test]
fn terminal_replay_limit_marks_truncated() {
    let session_id = Uuid::new_v4();
    let job_id = Uuid::new_v4();
    let outputs = vec![
        replay_chunk(job_id, "edge-a", session_id, 1, "100", b"one"),
        replay_chunk(job_id, "edge-a", session_id, 2, "100", b"two"),
    ];

    let replay =
        build_terminal_replay_from_chunks("edge-a", session_id, outputs, 1, 1, 1000, true, 3);

    assert_eq!(replay.chunk_count, 1);
    assert_eq!(replay.byte_count, 3);
    assert!(replay.truncated);
}

#[test]
fn terminal_replay_metadata_only_omits_data_and_applies_byte_cap() {
    let session_id = Uuid::new_v4();
    let job_id = Uuid::new_v4();
    let outputs = vec![
        replay_chunk(job_id, "edge-a", session_id, 1, "100", b"one"),
        replay_chunk(job_id, "edge-a", session_id, 2, "101", b"two"),
    ];

    let replay =
        build_terminal_replay_from_chunks("edge-a", session_id, outputs, 1, 10, 3, false, 3);

    assert_eq!(replay.chunk_count, 1);
    assert_eq!(replay.byte_count, 3);
    assert!(replay.truncated);
    assert_eq!(replay.chunks[0].terminal_seq, 1);
    assert!(replay.chunks[0].data_base64.is_none());
}

#[tokio::test]
async fn memory_terminal_control_ack_is_atomic_and_exactly_replayable() {
    let memory = MemoryState::default();
    let repo = Repository::Memory(memory.clone());
    let operator = terminal_test_operator();
    let client_id = "memory-terminal-control-atomic";
    let job_id = Uuid::new_v4();
    let session_id = Uuid::new_v4();
    memory
        .job_targets
        .write()
        .await
        .push(terminal_test_target(job_id, client_id, "running", None));
    memory
        .terminal_sessions
        .write()
        .await
        .push(test_terminal_session(
            client_id, session_id, job_id, 0, "open",
        ));
    let action = TerminalControlAction::Close {
        reason: Some("operator".to_string()),
    };
    let ack = TerminalControlAck {
        request_id: Uuid::new_v4(),
        session_id,
        action: "close".to_string(),
        accepted: true,
        status: "closed".to_string(),
        message: "closed".to_string(),
        input_seq: None,
        written_bytes: None,
        cols: None,
        rows: None,
    };

    let failed = repo
        .record_terminal_control_ack(
            &operator,
            client_id,
            job_id,
            &action,
            "memory-terminal-close-action-hash",
            &ack,
        )
        .await
        .unwrap_err();
    assert!(format!("{failed:#}").contains("terminal_open_job_not_found"));
    assert_eq!(memory.terminal_sessions.read().await[0].state, "open");
    assert_eq!(memory.job_targets.read().await[0].status, "running");
    assert!(memory.job_targets.read().await[0].completed_at.is_none());
    assert!(memory.audits.read().await.is_empty());

    memory.jobs.write().await.push(terminal_test_job(
        job_id,
        operator.operator.id,
        "running",
        None,
    ));
    repo.record_terminal_control_ack(
        &operator,
        client_id,
        job_id,
        &action,
        "memory-terminal-close-action-hash",
        &ack,
    )
    .await
    .unwrap();
    let first_observed_at = memory.terminal_sessions.read().await[0].observed_at.clone();
    assert_eq!(memory.terminal_sessions.read().await[0].state, "closed");
    assert_eq!(memory.job_targets.read().await[0].status, "completed");
    assert!(memory.job_targets.read().await[0].completed_at.is_some());
    assert_eq!(memory.jobs.read().await[0].status, "completed");
    assert!(memory.jobs.read().await[0].completed_at.is_some());
    assert_eq!(memory.audits.read().await.len(), 1);

    repo.record_terminal_control_ack(
        &operator,
        client_id,
        job_id,
        &action,
        "memory-terminal-close-action-hash",
        &ack,
    )
    .await
    .unwrap();
    assert_eq!(
        memory.terminal_sessions.read().await[0].observed_at,
        first_observed_at
    );
    assert_eq!(memory.audits.read().await.len(), 1);
}

#[tokio::test]
async fn memory_terminal_status_rolls_back_on_missing_target_then_replays() {
    let memory = MemoryState::default();
    let repo = Repository::Memory(memory.clone());
    let operator = terminal_test_operator();
    let client_id = "memory-terminal-status-atomic";
    let job_id = Uuid::new_v4();
    let session_id = Uuid::new_v4();
    memory.jobs.write().await.push(terminal_test_job(
        job_id,
        operator.operator.id,
        "running",
        None,
    ));
    let event = terminal_open_stream(job_id, session_id);

    let failed = repo
        .record_terminal_stream_status(client_id, &event)
        .await
        .unwrap_err();
    assert!(format!("{failed:#}").contains("terminal_open_target_not_found"));
    assert!(memory.terminal_sessions.read().await.is_empty());
    assert_eq!(memory.jobs.read().await[0].status, "running");
    assert!(memory.jobs.read().await[0].completed_at.is_none());

    memory
        .job_targets
        .write()
        .await
        .push(terminal_test_target(job_id, client_id, "queued", None));
    repo.record_terminal_stream_status(client_id, &event)
        .await
        .unwrap();
    repo.record_terminal_stream_status(client_id, &event)
        .await
        .unwrap();
    assert_eq!(memory.terminal_sessions.read().await.len(), 1);
    assert_eq!(memory.terminal_sessions.read().await[0].state, "open");
    assert_eq!(memory.jobs.read().await[0].status, "running");
    assert_eq!(memory.job_targets.read().await[0].status, "running");
}

#[tokio::test]
async fn memory_terminal_late_open_is_suppressed_and_stale_control_is_rejected() {
    let memory = MemoryState::default();
    let repo = Repository::Memory(memory.clone());
    let operator = terminal_test_operator();
    let client_id = "memory-terminal-stale";
    let job_id = Uuid::new_v4();
    let session_id = Uuid::new_v4();
    memory.jobs.write().await.push(terminal_test_job(
        job_id,
        operator.operator.id,
        "canceled",
        Some("2026-06-21T00:00:01Z"),
    ));
    memory.job_targets.write().await.push(terminal_test_target(
        job_id,
        client_id,
        "canceled",
        Some("2026-06-21T00:00:01Z"),
    ));
    memory
        .terminal_sessions
        .write()
        .await
        .push(test_terminal_session(
            client_id, session_id, job_id, 0, "open",
        ));

    let authorization = repo
        .authorize_terminal_control(client_id, session_id, &operator)
        .await
        .unwrap_err();
    assert_eq!(authorization.code, "terminal_session_job_inactive");
    let resize = TerminalControlAction::Resize {
        cols: 100,
        rows: 30,
    };
    let ack = TerminalControlAck {
        request_id: Uuid::new_v4(),
        session_id,
        action: "resize".to_string(),
        accepted: true,
        status: "resized".to_string(),
        message: "resized".to_string(),
        input_seq: None,
        written_bytes: None,
        cols: Some(100),
        rows: Some(30),
    };
    let rejected = repo
        .record_terminal_control_ack(
            &operator,
            client_id,
            job_id,
            &resize,
            "memory-terminal-resize-action-hash",
            &ack,
        )
        .await
        .unwrap_err();
    assert!(format!("{rejected:#}").contains("terminal_session_job_inactive"));
    assert_eq!(memory.terminal_sessions.read().await[0].cols, Some(120));

    repo.record_terminal_stream_status(client_id, &terminal_open_stream(job_id, session_id))
        .await
        .unwrap();
    assert_eq!(memory.terminal_sessions.read().await[0].state, "failed");
    assert_eq!(memory.jobs.read().await[0].status, "canceled");
    assert_eq!(memory.job_targets.read().await[0].status, "canceled");

    repo.record_terminal_stream_status(client_id, &terminal_open_stream(job_id, Uuid::new_v4()))
        .await
        .unwrap();
    assert_eq!(memory.terminal_sessions.read().await.len(), 1);
    assert_eq!(memory.terminal_sessions.read().await[0].state, "failed");
}

fn status_output(
    job_id: Uuid,
    client_id: &str,
    seq: i32,
    created_at: &str,
    value: serde_json::Value,
) -> TerminalStatusOutput {
    TerminalStatusOutput {
        job_id,
        client_id: client_id.to_string(),
        seq,
        data: serde_json::to_vec(&value).unwrap(),
        created_at: created_at.to_string(),
    }
}

fn replay_chunk(
    job_id: Uuid,
    client_id: &str,
    session_id: Uuid,
    terminal_seq: i64,
    created_at: &str,
    data: &[u8],
) -> TerminalOutputChunkRecord {
    TerminalOutputChunkRecord {
        client_id: client_id.to_string(),
        session_id,
        terminal_seq,
        job_id,
        data: data.to_vec(),
        size_bytes: data.len() as i64,
        sha256_hex: vpsman_common::payload_hash(data),
        created_at: created_at.to_string(),
    }
}

fn test_terminal_session(
    client_id: &str,
    session_id: Uuid,
    job_id: Uuid,
    last_input_seq: i64,
    state: &str,
) -> TerminalSessionView {
    TerminalSessionView {
        session_id,
        client_id: client_id.to_string(),
        job_id,
        state: state.to_string(),
        last_status: if state == "open" {
            "accepted"
        } else {
            "closed"
        }
        .to_string(),
        argv: vec!["/bin/sh".to_string(), "-l".to_string()],
        cwd: Some("/root".to_string()),
        cols: Some(120),
        rows: Some(40),
        idle_timeout_secs: Some(3600),
        flow_window_bytes: Some(65_536),
        output_first_seq: Some(1),
        output_next_seq: Some(1),
        output_retained_first_seq: Some(1),
        output_retained_bytes: Some(0),
        output_dropped_bytes: Some(0),
        output_dropped_chunks: Some(0),
        output_replay_truncated: false,
        last_input_seq,
        close_reason: None,
        last_event: state.to_string(),
        opened_at: Some("2026-06-21T00:00:00Z".to_string()),
        observed_at: "2026-06-21T00:00:00Z".to_string(),
    }
}

fn terminal_test_operator() -> AuthContext {
    AuthContext {
        operator: OperatorView {
            id: Uuid::new_v4(),
            username: "terminal-test-operator".to_string(),
            status: "active".to_string(),
            role: "admin".to_string(),
            scopes: vec!["*".to_string()],
            preferences: OperatorPreferences::default(),
            totp_enabled: false,
            session_refresh_ttl_secs: 3600,
            created_at: "2026-06-21T00:00:00Z".to_string(),
            disabled_at: None,
            deleted_at: None,
        },
        session_id: None,
    }
}

fn terminal_test_job(
    job_id: Uuid,
    actor_id: Uuid,
    status: &str,
    completed_at: Option<&str>,
) -> JobHistoryView {
    JobHistoryView {
        id: job_id,
        actor_id: Some(actor_id),
        command_type: "terminal_open".to_string(),
        source_schedule_id: None,
        causation_id: None,
        schedule_lineage: Vec::new(),
        privileged: true,
        status: status.to_string(),
        target_count: 1,
        payload_hash: "terminal-test".to_string(),
        max_timeout_secs: 60,
        created_at: "2026-06-21T00:00:00Z".to_string(),
        completed_at: completed_at.map(str::to_string),
    }
}

fn terminal_test_target(
    job_id: Uuid,
    client_id: &str,
    status: &str,
    completed_at: Option<&str>,
) -> JobTargetView {
    JobTargetView {
        job_id,
        client_id: client_id.to_string(),
        status: status.to_string(),
        message: None,
        exit_code: None,
        started_at: Some("2026-06-21T00:00:00Z".to_string()),
        deadline_at: None,
        completed_at: completed_at.map(str::to_string),
        process_incarnation_id: None,
    }
}

fn terminal_open_stream(job_id: Uuid, session_id: Uuid) -> TerminalStreamOutput {
    TerminalStreamOutput {
        job_id,
        session_id,
        terminal_seq: None,
        output_first_seq: Some(1),
        output_next_seq: 1,
        output_retained_first_seq: Some(1),
        output_retained_bytes: 0,
        output_dropped_bytes: 0,
        output_dropped_chunks: 0,
        output_replay_truncated: false,
        output: CommandOutput {
            job_id,
            stream: OutputStream::Status,
            data: serde_json::to_vec(&serde_json::json!({
                "type": "terminal_open",
                "status": "opened",
                "session_id": session_id,
                "argv": ["/bin/sh", "-l"],
                "cols": 120,
                "rows": 40,
                "idle_timeout_secs": 3600,
                "flow_window_bytes": 65536,
                "output_first_seq": 1,
                "output_next_seq": 1,
                "session_exited": false
            }))
            .unwrap(),
            exit_code: None,
            done: false,
        },
    }
}
