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
#[path = "tests_vty_network_observations.rs"]
mod tests;
