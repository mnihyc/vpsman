use super::{terminal_replay_path, terminal_sessions_path};
use uuid::Uuid;

#[test]
fn builds_filtered_terminal_sessions_path() {
    let path = terminal_sessions_path(
        500,
        Some("edge a"),
        Some("11111111-2222-4333-8444-555555555555"),
    )
    .unwrap();

    assert_eq!(
        path,
        "/api/v1/terminal-sessions?limit=200&client_id=edge%20a&session_id=11111111-2222-4333-8444-555555555555"
    );
}

#[test]
fn builds_terminal_replay_path() {
    let session_id = Uuid::parse_str("11111111-2222-4333-8444-555555555555").unwrap();

    assert_eq!(
        terminal_replay_path("edge a", session_id, Some(7), 5000, 0, false),
        "/api/v1/terminal-sessions/edge%20a/11111111-2222-4333-8444-555555555555/replay?limit=1000&max_bytes=1&include_data=true&from_seq=7"
    );
}
