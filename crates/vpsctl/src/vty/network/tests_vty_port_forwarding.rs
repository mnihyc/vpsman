use super::*;

#[test]
fn recognizes_mutating_and_resolve_commands() {
    assert!(is_vty_port_forward_command(
        "port-forward-resolve --hostname example.com"
    ));
    assert!(is_vty_port_forward_command(
        "port-forward-enable --rule-id 00000000-0000-0000-0000-000000000000 --expected-revision 1 --confirmed"
    ));
    assert!(!is_vty_port_forward_command("port-forwards"));
    assert!(matches!(
        VtyPortForwardArgs::try_parse_from([
            "vty",
            "port-forward-resolve",
            "--hostname",
            "example.com",
        ])
        .unwrap()
        .command,
        VtyPortForwardCommand::Resolve(_)
    ));
}
