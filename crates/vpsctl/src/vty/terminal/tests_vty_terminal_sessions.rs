use super::is_vty_terminal_sessions_command;

#[test]
fn recognizes_terminal_sessions_commands() {
    assert!(is_vty_terminal_sessions_command("terminal-sessions"));
    assert!(is_vty_terminal_sessions_command(
        "terminal-sessions --limit 10"
    ));
    assert!(is_vty_terminal_sessions_command(
        "terminal-replay --client-id edge-a --session-id 11111111-2222-4333-8444-555555555555"
    ));
    assert!(is_vty_terminal_sessions_command(
        "terminal-follow --client-id edge-a --session-id 11111111-2222-4333-8444-555555555555"
    ));
    assert!(!is_vty_terminal_sessions_command(
        "terminal-open --argv /bin/sh"
    ));
}
