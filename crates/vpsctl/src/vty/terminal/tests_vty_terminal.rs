use super::{parse_vty_terminal, VtyTerminalRequest};
use vpsman_common::JobCommand;

const TEST_TERMINAL_ARGV: &str = "/bin/sh,-l";

#[test]
fn parses_terminal_open_contract() {
    let request = parse_vty_terminal(
        "terminal-open",
        &[
            "--argv",
            TEST_TERMINAL_ARGV,
            "--cols",
            "100",
            "--rows",
            "30",
            "id:edge-a",
            "--confirmed",
        ],
    )
    .unwrap();
    match request {
        VtyTerminalRequest::Job {
            command_label,
            operation,
            selection,
            ..
        } => {
            assert_eq!(command_label, "terminal_open");
            assert!(selection.clients.is_empty());
            assert_eq!(selection.tags, vec!["id:edge-a".to_string()]);
            assert!(selection.confirmed);
            match *operation {
                JobCommand::TerminalOpen {
                    argv, cols, rows, ..
                } => {
                    assert_eq!(argv, vec!["/bin/sh".to_string(), "-l".to_string()]);
                    assert_eq!(cols, 100);
                    assert_eq!(rows, 30);
                }
                other => panic!("unexpected operation {other:?}"),
            }
        }
        other => panic!("unexpected operation {other:?}"),
    }
}

#[test]
fn parses_terminal_input_text() {
    let request = parse_vty_terminal(
        "terminal-input",
        &[
            "--session-id",
            "11111111-1111-4111-8111-111111111111",
            "--client-id",
            "edge-a",
            "--text",
            "id",
        ],
    )
    .unwrap();
    match request {
        VtyTerminalRequest::Control {
            client_id,
            session_id,
            action,
        } => {
            assert_eq!(client_id, "edge-a");
            assert_eq!(
                session_id.to_string(),
                "11111111-1111-4111-8111-111111111111"
            );
            assert_eq!(
                action,
                vpsman_common::TerminalControlAction::Input {
                    data_base64: "aWQ=".to_string()
                }
            );
        }
        other => panic!("unexpected request {other:?}"),
    }
}

#[test]
fn rejects_removed_terminal_input_seq() {
    assert!(parse_vty_terminal(
        "terminal-input",
        &[
            "--session-id",
            "11111111-1111-4111-8111-111111111111",
            "--client-id",
            "edge-a",
            "--input-seq",
            "7",
            "--text",
            "id",
        ],
    )
    .is_err());
}

#[test]
fn parses_terminal_poll() {
    let request = parse_vty_terminal(
        "terminal-poll",
        &[
            "--session-id",
            "11111111-1111-4111-8111-111111111111",
            "--replay-from-seq",
            "4",
            "--client-id",
            "edge-a",
        ],
    )
    .unwrap();
    match request {
        VtyTerminalRequest::Replay {
            client_id,
            session_id,
            from_seq,
        } => {
            assert_eq!(client_id, "edge-a");
            assert_eq!(
                session_id.to_string(),
                "11111111-1111-4111-8111-111111111111"
            );
            assert_eq!(from_seq, Some(4));
        }
        other => panic!("unexpected request {other:?}"),
    }
}

#[test]
fn parses_terminal_resize_and_close() {
    let resize = parse_vty_terminal(
        "terminal-resize",
        &[
            "--session-id",
            "11111111-1111-4111-8111-111111111111",
            "--cols",
            "90",
            "--rows",
            "24",
            "--client-id",
            "edge-a",
        ],
    )
    .unwrap();
    match resize {
        VtyTerminalRequest::Control {
            client_id, action, ..
        } => {
            assert_eq!(client_id, "edge-a");
            match action {
                vpsman_common::TerminalControlAction::Resize { cols, rows } => {
                    assert_eq!(cols, 90);
                    assert_eq!(rows, 24);
                }
                other => panic!("unexpected operation {other:?}"),
            }
        }
        other => panic!("unexpected request {other:?}"),
    }
    let close = parse_vty_terminal(
        "terminal-close",
        &[
            "--session-id",
            "11111111-1111-4111-8111-111111111111",
            "--reason",
            "done",
            "--client-id",
            "edge-a",
        ],
    )
    .unwrap();
    match close {
        VtyTerminalRequest::Control {
            client_id, action, ..
        } => {
            assert_eq!(client_id, "edge-a");
            match action {
                vpsman_common::TerminalControlAction::Close { reason } => {
                    assert_eq!(reason.as_deref(), Some("done"));
                }
                other => panic!("unexpected operation {other:?}"),
            }
        }
        other => panic!("unexpected request {other:?}"),
    }
}
