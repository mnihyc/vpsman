use super::is_vty_network_dispatch_command;

#[test]
fn recognizes_network_dispatch_commands() {
    for command in [
        "tunnel-plan --name n",
        "tunnel-plan-export --plan-id 00000000-0000-0000-0000-000000000000",
        "tunnel-plan-enable --plan-id 00000000-0000-0000-0000-000000000000 --expected-revision 1 --confirmed",
        "tunnel-plan-disable --plan-id 00000000-0000-0000-0000-000000000000 --expected-revision 1 --confirmed",
        "tunnel-plan-rotate-credentials --plan-id 00000000-0000-0000-0000-000000000000 --expected-revision 1 --confirmed",
        "tunnel-plan-delete --plan-id 00000000-0000-0000-0000-000000000000 --expected-revision 1 --confirmed",
        "tunnel-allocate --ipv4-pool-cidr 10.255.0.0/24",
        "tunnel-ospf-status-refresh --plan-id 00000000-0000-0000-0000-000000000000",
        "tunnel-ospf-cost-update --plan-id 00000000-0000-0000-0000-000000000000",
        "tunnel-status --plan-id <uuid>",
        "tunnel-probe --plan-id <uuid>",
        "tunnel-speed-test --plan-id <uuid>",
    ] {
        assert!(is_vty_network_dispatch_command(command), "{command}");
    }

    for command in [
        "tunnel-plans",
        "network-observations",
        "job-create uptime id:edge",
        "tunnel-plan",
    ] {
        assert!(!is_vty_network_dispatch_command(command), "{command}");
    }
}
