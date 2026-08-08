use super::{is_vty_network_evidence_command, submit_vty_network_evidence_command};

#[test]
fn recognizes_read_only_network_evidence_commands() {
    assert!(is_vty_network_evidence_command("network-observations"));
    assert!(is_vty_network_evidence_command("network-trends --limit 20"));
    assert!(is_vty_network_evidence_command(
        "network-ospf-recommendations --limit=20"
    ));
    assert!(is_vty_network_evidence_command(
        "network-ospf-update-plans --limit=20"
    ));
    assert!(is_vty_network_evidence_command("topology-graph --limit=20"));
    assert!(!is_vty_network_evidence_command("network-evidence-clear"));
    assert!(!is_vty_network_evidence_command("network-probe --limit=20"));
}

#[test]
fn network_evidence_usage_errors_are_non_fatal() {
    let usage = submit_vty_network_evidence_command(
        "http://127.0.0.1:1",
        None,
        "network-ospf-recommendations --bad",
    )
    .unwrap();
    let bad_limit = submit_vty_network_evidence_command(
        "http://127.0.0.1:1",
        None,
        "network-trends --limit bad",
    )
    .unwrap();
    let update_plan_usage = submit_vty_network_evidence_command(
        "http://127.0.0.1:1",
        None,
        "network-ospf-update-plans --bad",
    )
    .unwrap();
    let topology_graph_usage =
        submit_vty_network_evidence_command("http://127.0.0.1:1", None, "topology-graph --bad")
            .unwrap();

    assert_eq!(
        usage,
        "usage: network-ospf-recommendations [--limit <1-200>]"
    );
    assert_eq!(bad_limit, "usage error: --limit must be an integer: bad");
    assert_eq!(
        update_plan_usage,
        "usage: network-ospf-update-plans [--limit <1-200>]"
    );
    assert_eq!(
        topology_graph_usage,
        "usage: topology-graph [--limit <1-200>]"
    );
}
