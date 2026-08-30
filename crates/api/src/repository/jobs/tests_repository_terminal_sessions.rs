use super::{
    build_terminal_replay_from_chunks, parse_terminal_command_replay_status, parse_terminal_event,
    terminal_command_replay_plan, upsert_test_terminal_session, TerminalStatusOutput,
};
use crate::model::JobOutputView;
use crate::model_terminal::{TerminalOutputChunkRecord, TerminalSessionView};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use uuid::Uuid;

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

    upsert_test_terminal_session(
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

#[test]
fn terminal_command_replay_plan_maps_only_the_status_owned_pty_range() {
    let session_id = Uuid::new_v4();
    let output = persisted_status_output(
        7,
        serde_json::json!({
            "type": "terminal_open",
            "status": "attached",
            "session_id": session_id,
            "output_first_seq": 5,
            "output_next_seq": 8,
            "output_retained_first_seq": 3,
            "output_retained_bytes": 128,
            "output_dropped_bytes": 64,
            "output_dropped_chunks": 1,
            "output_replay_truncated": true
        }),
    );

    let status = parse_terminal_command_replay_status(&output).unwrap();
    let plan = terminal_command_replay_plan(output.seq, &status)
        .unwrap()
        .unwrap();

    assert_eq!(status.session_id, session_id);
    assert_eq!(plan.first_job_output_seq, 4);
    assert_eq!(plan.first_terminal_seq, 5);
    assert_eq!(plan.next_terminal_seq, 8);
    assert_eq!(plan.output_count, 3);
}

#[test]
fn terminal_command_replay_plan_rejects_a_status_before_its_source_range() {
    let output = persisted_status_output(
        1,
        serde_json::json!({
            "type": "terminal_open",
            "status": "opened",
            "session_id": Uuid::new_v4(),
            "output_first_seq": 1,
            "output_next_seq": 4
        }),
    );
    let status = parse_terminal_command_replay_status(&output).unwrap();

    assert!(terminal_command_replay_plan(output.seq, &status).is_err());
}

#[test]
fn terminal_command_replay_ignores_live_stream_statuses() {
    let output = persisted_status_output(
        0,
        serde_json::json!({
            "type": "terminal_stream",
            "status": "streaming",
            "session_id": Uuid::new_v4(),
            "output_first_seq": 1,
            "output_next_seq": 2
        }),
    );

    assert!(parse_terminal_command_replay_status(&output).is_none());
}

fn persisted_status_output(seq: i32, value: serde_json::Value) -> JobOutputView {
    JobOutputView {
        job_id: Uuid::new_v4(),
        client_id: "edge-a".to_string(),
        seq,
        stream: "status".to_string(),
        data_base64: BASE64.encode(serde_json::to_vec(&value).unwrap()),
        storage: "inline".to_string(),
        artifact_object_key: None,
        artifact_sha256_hex: None,
        artifact_size_bytes: None,
        exit_code: Some(0),
        done: false,
        received_at: None,
        created_at: "2026-08-28T00:00:00Z".to_string(),
    }
}

fn status_output(
    job_id: Uuid,
    client_id: &str,
    created_at: &str,
    value: serde_json::Value,
) -> TerminalStatusOutput {
    TerminalStatusOutput {
        job_id,
        client_id: client_id.to_string(),
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
