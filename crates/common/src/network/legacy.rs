use super::models::{
    IfupdownConfig, IfupdownInterface, LegacyBirdConfig, LegacyBirdPeer, TunnelKind,
};

pub fn parse_legacy_bird_config(predef_conf: &str, ospf_conf: &str) -> LegacyBirdConfig {
    let router_id = predef_conf.lines().find_map(|line| {
        let trimmed = line.trim();
        trimmed
            .strip_prefix("router id ")
            .and_then(|value| value.trim_end_matches(';').split_whitespace().next())
            .map(ToOwned::to_owned)
    });

    let mut protocol_name = String::new();
    let mut node_name = None::<String>;
    let mut peers = Vec::new();
    let mut current_interface = None::<String>;
    let mut current_cost = None::<u16>;
    let mut current_is_ptp = false;

    for line in ospf_conf.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("protocol ospf") {
            protocol_name = trimmed
                .trim_end_matches('{')
                .split_whitespace()
                .last()
                .unwrap_or("ospf")
                .to_string();
            node_name = Some(protocol_name.clone());
        }

        if trimmed.starts_with("interface ") {
            flush_peer(
                &mut peers,
                &protocol_name,
                &mut current_interface,
                &mut current_cost,
                &mut current_is_ptp,
            );
            current_interface = extract_quoted(trimmed);
        } else if trimmed.starts_with("cost ") {
            current_cost = trimmed
                .trim_start_matches("cost ")
                .trim_end_matches(';')
                .parse::<u16>()
                .ok();
        } else if trimmed == "type ptp;" {
            current_is_ptp = true;
        } else if trimmed.starts_with('}') {
            flush_peer(
                &mut peers,
                &protocol_name,
                &mut current_interface,
                &mut current_cost,
                &mut current_is_ptp,
            );
        }
    }

    flush_peer(
        &mut peers,
        &protocol_name,
        &mut current_interface,
        &mut current_cost,
        &mut current_is_ptp,
    );

    LegacyBirdConfig {
        router_id,
        node_name,
        peers,
    }
}

fn flush_peer(
    peers: &mut Vec<LegacyBirdPeer>,
    protocol_name: &str,
    current_interface: &mut Option<String>,
    current_cost: &mut Option<u16>,
    current_is_ptp: &mut bool,
) {
    let Some(interface_name) = current_interface.take() else {
        return;
    };
    if *current_is_ptp {
        let peer_name = interface_name
            .split_once("peer")
            .map(|(_, peer)| peer.to_string())
            .filter(|peer| !peer.is_empty());
        peers.push(LegacyBirdPeer {
            protocol_name: protocol_name.to_string(),
            interface_name,
            peer_name,
            cost: *current_cost,
        });
    }
    *current_cost = None;
    *current_is_ptp = false;
}

fn extract_quoted(value: &str) -> Option<String> {
    let start = value.find('"')?;
    let rest = &value[start + 1..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

pub fn parse_ifupdown_configs(configs: &[(&str, &str)]) -> IfupdownConfig {
    let mut parsed = IfupdownConfig::default();
    for (source_path, contents) in configs {
        parsed
            .interfaces
            .extend(parse_ifupdown_config(source_path, contents));
    }
    parsed
}

fn parse_ifupdown_config(source_path: &str, contents: &str) -> Vec<IfupdownInterface> {
    let mut interfaces = Vec::new();
    let mut current = None::<IfupdownInterface>;

    for raw_line in contents.lines() {
        let trimmed = raw_line
            .split_once('#')
            .map(|(line, _)| line)
            .unwrap_or(raw_line)
            .trim();
        if trimmed.is_empty() {
            continue;
        }

        let fields = trimmed.split_whitespace().collect::<Vec<_>>();
        if fields.first() == Some(&"iface") && fields.len() >= 2 {
            if let Some(interface) = current.take() {
                interfaces.push(interface);
            }
            current = Some(IfupdownInterface {
                source_path: source_path.to_string(),
                name: fields[1].to_string(),
                address: None,
                point_to_point: None,
                tunnel_kind: None,
                tunnel_local: None,
                tunnel_remote: None,
            });
            continue;
        }

        let Some(interface) = current.as_mut() else {
            continue;
        };

        match fields.as_slice() {
            ["address", address, ..] => interface.address = Some((*address).to_string()),
            ["pointopoint", peer, ..] | ["point-to-point", peer, ..] => {
                interface.point_to_point = Some((*peer).to_string());
            }
            _ => {
                if let Some(tunnel) = parse_ip_tunnel_command(&fields) {
                    interface.tunnel_kind = Some(tunnel.kind);
                    interface.tunnel_local = tunnel.local;
                    interface.tunnel_remote = tunnel.remote;
                }
            }
        }
    }

    if let Some(interface) = current {
        interfaces.push(interface);
    }

    interfaces
}

#[derive(Debug)]
struct ParsedTunnelCommand {
    kind: TunnelKind,
    local: Option<String>,
    remote: Option<String>,
}

fn parse_ip_tunnel_command(fields: &[&str]) -> Option<ParsedTunnelCommand> {
    let ip_index = fields.iter().position(|field| *field == "ip")?;
    if fields.get(ip_index + 1) != Some(&"tunnel") || fields.get(ip_index + 2) != Some(&"add") {
        return None;
    }

    let mut kind = None::<TunnelKind>;
    let mut local = None::<String>;
    let mut remote = None::<String>;
    let mut uses_fou = false;
    let mut index = ip_index + 3;
    while index < fields.len() {
        match fields[index] {
            "mode" => {
                if let Some(value) = fields.get(index + 1) {
                    kind = match *value {
                        "gre" | "gretap" => Some(TunnelKind::Gre),
                        "ipip" => Some(TunnelKind::Ipip),
                        "sit" => Some(TunnelKind::Sit),
                        _ => kind,
                    };
                }
                index += 2;
            }
            "local" => {
                local = fields.get(index + 1).map(|value| (*value).to_string());
                index += 2;
            }
            "remote" => {
                remote = fields.get(index + 1).map(|value| (*value).to_string());
                index += 2;
            }
            "encap" => {
                uses_fou = fields.get(index + 1) == Some(&"fou");
                index += 2;
            }
            _ => index += 1,
        }
    }

    let kind = match (kind, uses_fou) {
        (Some(TunnelKind::Ipip), true) => TunnelKind::Fou,
        (Some(kind), _) => kind,
        (None, true) => TunnelKind::Fou,
        (None, false) => return None,
    };
    Some(ParsedTunnelCommand {
        kind,
        local,
        remote,
    })
}
