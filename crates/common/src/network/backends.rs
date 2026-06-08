use super::{
    models::{
        RuntimeTunnelManager, TunnelBackendConfig, TunnelBackendFile, TunnelConfigBackend,
        TunnelEndpointConfig, TunnelEndpointSide, TunnelKind, TunnelPlan, MANAGED_IFUPDOWN_FILE,
        MANAGED_NETPLAN_FILE, MANAGED_SYSTEMD_NETWORKD_NETDEV_FILE,
        MANAGED_SYSTEMD_NETWORKD_NETWORK_FILE,
    },
    planner::{render_tunnel_endpoint_config, NetworkPlanError},
};

pub fn render_tunnel_endpoint_backend_config(
    plan: &TunnelPlan,
    side: TunnelEndpointSide,
    backend: TunnelConfigBackend,
) -> Result<TunnelBackendConfig, NetworkPlanError> {
    let endpoint = render_tunnel_endpoint_config(plan, side)?;
    render_backend_config_for_endpoint(plan, &endpoint, backend)
}

pub fn render_backend_config_for_endpoint(
    plan: &TunnelPlan,
    endpoint: &TunnelEndpointConfig,
    backend: TunnelConfigBackend,
) -> Result<TunnelBackendConfig, NetworkPlanError> {
    if plan.runtime_control.manager != RuntimeTunnelManager::AgentIproute2Managed {
        return Ok(TunnelBackendConfig {
            backend,
            files: Vec::new(),
        });
    }

    let files = match backend {
        TunnelConfigBackend::Ifupdown => vec![TunnelBackendFile {
            managed_path: MANAGED_IFUPDOWN_FILE,
            block_kind: "ifupdown",
            contents: endpoint.ifupdown_snippet.clone(),
        }],
        TunnelConfigBackend::Netplan => vec![TunnelBackendFile {
            managed_path: MANAGED_NETPLAN_FILE,
            block_kind: "netplan",
            contents: render_netplan_tunnel(plan, endpoint)?,
        }],
        TunnelConfigBackend::SystemdNetworkd => vec![
            TunnelBackendFile {
                managed_path: MANAGED_SYSTEMD_NETWORKD_NETDEV_FILE,
                block_kind: "systemd_networkd_netdev",
                contents: render_systemd_netdev(plan, endpoint)?,
            },
            TunnelBackendFile {
                managed_path: MANAGED_SYSTEMD_NETWORKD_NETWORK_FILE,
                block_kind: "systemd_networkd_network",
                contents: render_systemd_network(plan, endpoint),
            },
        ],
    };
    Ok(TunnelBackendConfig { backend, files })
}

pub fn backend_config_signature_payload(config: &TunnelBackendConfig) -> Vec<u8> {
    let mut payload = String::new();
    for file in &config.files {
        payload.push_str("vpsman-network-backend-file-v1\n");
        payload.push_str("backend=");
        payload.push_str(config.backend.as_str());
        payload.push('\n');
        payload.push_str("path=");
        payload.push_str(file.managed_path);
        payload.push('\n');
        payload.push_str("kind=");
        payload.push_str(file.block_kind);
        payload.push('\n');
        payload.push_str("contents-sha256-context\n");
        payload.push_str(&file.contents);
        payload.push('\n');
    }
    payload.into_bytes()
}

impl TunnelConfigBackend {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ifupdown => "ifupdown",
            Self::Netplan => "netplan",
            Self::SystemdNetworkd => "systemd_networkd",
        }
    }
}

fn render_netplan_tunnel(
    plan: &TunnelPlan,
    endpoint: &TunnelEndpointConfig,
) -> Result<String, NetworkPlanError> {
    let mode = match plan.kind {
        TunnelKind::Gre => "gre",
        TunnelKind::Ipip => "ipip",
        TunnelKind::Sit => "sit",
        TunnelKind::Fou
        | TunnelKind::Openvpn
        | TunnelKind::Wireguard
        | TunnelKind::TunTap
        | TunnelKind::Custom => return Err(NetworkPlanError::UnsupportedBackendTunnelKind),
    };
    Ok(format!(
        "\
# vpsman tunnel {}: generated endpoint {}
network:
  version: 2
  renderer: networkd
  tunnels:
    {}:
      mode: {}
      local: {}
      remote: {}
      ttl: 255
      addresses:
        - {}/{}
",
        plan.name,
        endpoint.local_client_id,
        plan.interface_name,
        mode,
        local_underlay(plan, endpoint),
        remote_underlay(plan, endpoint),
        local_address(plan, endpoint),
        plan.tunnel_prefix_len
    ))
}

fn render_systemd_netdev(
    plan: &TunnelPlan,
    endpoint: &TunnelEndpointConfig,
) -> Result<String, NetworkPlanError> {
    let (kind, extra) = match plan.kind {
        TunnelKind::Gre => ("gre", String::new()),
        TunnelKind::Ipip => ("ipip", String::new()),
        TunnelKind::Sit => ("sit", String::new()),
        TunnelKind::Fou => (
            "fou",
            format!(
                "\n[FooOverUDP]\nEncapsulation=FooOverUDP\nPort={}\nPeerPort={}\nProtocol={}\n",
                plan.runtime_control.fou.port,
                plan.runtime_control.fou.peer_port,
                plan.runtime_control.fou.ipproto
            ),
        ),
        TunnelKind::Openvpn | TunnelKind::Wireguard | TunnelKind::TunTap | TunnelKind::Custom => {
            return Err(NetworkPlanError::UnsupportedBackendTunnelKind);
        }
    };
    Ok(format!(
        "\
# vpsman tunnel {}: generated endpoint {}
[NetDev]
Name={}
Kind={}

[Tunnel]
Local={}
Remote={}
TTL=255{}
",
        plan.name,
        endpoint.local_client_id,
        plan.interface_name,
        kind,
        local_underlay(plan, endpoint),
        remote_underlay(plan, endpoint),
        extra
    ))
}

fn render_systemd_network(plan: &TunnelPlan, endpoint: &TunnelEndpointConfig) -> String {
    format!(
        "\
# vpsman tunnel {}: generated endpoint {}
[Match]
Name={}

[Network]
Address={}/{}
Peer={}
",
        plan.name,
        endpoint.local_client_id,
        plan.interface_name,
        local_address(plan, endpoint),
        plan.tunnel_prefix_len,
        remote_address(plan, endpoint)
    )
}

fn endpoint_is_left(plan: &TunnelPlan, endpoint: &TunnelEndpointConfig) -> bool {
    endpoint.local_client_id == plan.left_client_id
}

fn local_underlay<'a>(plan: &'a TunnelPlan, endpoint: &TunnelEndpointConfig) -> &'a str {
    if endpoint_is_left(plan, endpoint) {
        &plan.left_underlay
    } else {
        &plan.right_underlay
    }
}

fn remote_underlay<'a>(plan: &'a TunnelPlan, endpoint: &TunnelEndpointConfig) -> &'a str {
    if endpoint_is_left(plan, endpoint) {
        &plan.right_underlay
    } else {
        &plan.left_underlay
    }
}

fn local_address<'a>(plan: &'a TunnelPlan, endpoint: &TunnelEndpointConfig) -> &'a str {
    if endpoint_is_left(plan, endpoint) {
        &plan.left_tunnel_address
    } else {
        &plan.right_tunnel_address
    }
}

fn remote_address<'a>(plan: &'a TunnelPlan, endpoint: &TunnelEndpointConfig) -> &'a str {
    if endpoint_is_left(plan, endpoint) {
        &plan.right_tunnel_address
    } else {
        &plan.left_tunnel_address
    }
}
