use anyhow::Result;

use crate::http::http_get;

struct NetworkEvidenceCommand {
    name: &'static str,
    endpoint: &'static str,
    usage: &'static str,
}

const NETWORK_EVIDENCE_COMMANDS: &[NetworkEvidenceCommand] = &[
    NetworkEvidenceCommand {
        name: "network-observations",
        endpoint: "/api/v1/network/observations",
        usage: "usage: network-observations [--limit <1-200>]",
    },
    NetworkEvidenceCommand {
        name: "network-trends",
        endpoint: "/api/v1/network/observation-trends",
        usage: "usage: network-trends [--limit <1-200>]",
    },
    NetworkEvidenceCommand {
        name: "network-ospf-recommendations",
        endpoint: "/api/v1/network/ospf-recommendations",
        usage: "usage: network-ospf-recommendations [--limit <1-200>]",
    },
    NetworkEvidenceCommand {
        name: "network-ospf-update-plans",
        endpoint: "/api/v1/network/ospf-update-plans",
        usage: "usage: network-ospf-update-plans [--limit <1-200>]",
    },
    NetworkEvidenceCommand {
        name: "topology-graph",
        endpoint: "/api/v1/network/topology-graph",
        usage: "usage: topology-graph [--limit <1-200>]",
    },
];

pub(crate) fn is_vty_network_evidence_command(command: &str) -> bool {
    NETWORK_EVIDENCE_COMMANDS
        .iter()
        .any(|spec| command_matches_name(command, spec.name))
}

pub(crate) fn submit_vty_network_evidence_command(
    api_url: &str,
    token: Option<&str>,
    command: &str,
) -> Result<String> {
    let spec = NETWORK_EVIDENCE_COMMANDS
        .iter()
        .find(|spec| command_matches_name(command, spec.name))
        .expect("caller checked command shape");
    let parts = command.split_whitespace().collect::<Vec<_>>();
    match parts.as_slice() {
        [name] if *name == spec.name => http_get(api_url, spec.endpoint, token),
        [name, "--limit", value] if *name == spec.name => {
            query_with_limit(api_url, token, spec, value)
        }
        [name, value] if *name == spec.name && value.starts_with("--limit=") => {
            query_with_limit(api_url, token, spec, value.trim_start_matches("--limit="))
        }
        _ => Ok(spec.usage.to_string()),
    }
}

fn command_matches_name(command: &str, name: &str) -> bool {
    command == name
        || command
            .strip_prefix(name)
            .is_some_and(|remaining| remaining.starts_with(' '))
}

fn query_with_limit(
    api_url: &str,
    token: Option<&str>,
    spec: &NetworkEvidenceCommand,
    value: &str,
) -> Result<String> {
    let Ok(limit) = value.parse::<u16>() else {
        return Ok(format!("usage error: --limit must be an integer: {value}"));
    };
    http_get(
        api_url,
        &format!("{}?limit={}", spec.endpoint, limit.clamp(1, 200)),
        token,
    )
}

#[cfg(test)]
mod tests {
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
}
