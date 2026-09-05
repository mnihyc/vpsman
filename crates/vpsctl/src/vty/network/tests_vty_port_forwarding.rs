use super::*;

#[test]
fn parses_redirect_and_custom_without_mandatory_remote_target() {
    let base = [
        "vty",
        "port-forward-create",
        "--client-id",
        "edge-a",
        "--name",
        "local",
        "--incoming",
        "443",
        "--target",
        "8443",
        "--disabled",
    ];
    let redirect = VtyPortForwardArgs::try_parse_from(base.into_iter().chain([
        "--mode",
        "redirect",
        "--address-family",
        "both",
    ]))
    .unwrap();
    assert!(
        matches!(redirect.command, VtyPortForwardCommand::Create(request)
        if request.target_ip.is_none()
        && matches!(request.forwarding.mode, commands_port_forwarding::PortForwardModeArg::Redirect))
    );
    let custom = VtyPortForwardArgs::try_parse_from(base.into_iter().chain([
        "--mode",
        "custom_adapter",
        "--adapter-definition-id",
        "00000000-0000-0000-0000-000000000001",
    ]))
    .unwrap();
    assert!(
        matches!(custom.command, VtyPortForwardCommand::Create(request)
        if request.forwarding.adapter_definition_id.is_some() && request.target_ip.is_none())
    );
}

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

#[test]
fn parses_target_hostname_set_and_clear_options() {
    let created = VtyPortForwardArgs::try_parse_from([
        "vty",
        "port-forward-create",
        "--client-id",
        "edge-a",
        "--name",
        "web",
        "--target-ip",
        "192.0.2.8",
        "--target-hostname",
        "app.internal",
        "--incoming",
        "443",
        "--target",
        "8443",
    ])
    .unwrap();
    assert!(matches!(
        created.command,
        VtyPortForwardCommand::Create(request)
            if request.target_hostname.as_deref() == Some("app.internal")
    ));

    let update_args = [
        "vty",
        "port-forward-update",
        "--rule-id",
        "00000000-0000-0000-0000-000000000001",
        "--expected-revision",
        "1",
        "--name",
        "web",
        "--protocol",
        "tcp",
        "--target-ip",
        "192.0.2.9",
        "--incoming",
        "443",
        "--target",
        "8443",
    ];
    let cleared = VtyPortForwardArgs::try_parse_from(
        update_args.into_iter().chain(["--clear-target-hostname"]),
    )
    .unwrap();
    assert!(matches!(
        cleared.command,
        VtyPortForwardCommand::Update(request)
            if request.clear_target_hostname && request.target_hostname.is_none()
    ));

    assert!(
        VtyPortForwardArgs::try_parse_from(update_args.into_iter().chain([
            "--target-hostname",
            "app.internal",
            "--clear-target-hostname",
        ]))
        .is_err()
    );
}
