use super::*;

use vpsman_common::{
    plan_tunnel, JobCommand, OspfControlMode, OspfCostPolicy, RuntimeTunnelAdapterCommands,
    RuntimeTunnelCommand, RuntimeTunnelControl, RuntimeTunnelManager, TunnelAddressFamily,
    TunnelAddressPair, TunnelEndpointSide, TunnelKind, TunnelOspfConfig, TunnelPlanInput,
};

use crate::job_request::validate_job_command;

const LEFT_RUNTIME_ADAPTER: &str = "11111111-1111-4111-8111-111111111111";
const RIGHT_RUNTIME_ADAPTER: &str = "22222222-2222-4222-8222-222222222222";
const LEFT_ROUTING_ADAPTER: &str = "33333333-3333-4333-8333-333333333333";
const RIGHT_ROUTING_ADAPTER: &str = "44444444-4444-4444-8444-444444444444";

#[test]
fn network_status_requires_a_server_bound_runtime_adapter_snapshot() {
    let plan = plan_tunnel(&test_plan_input(RuntimeTunnelManager::CustomAdapter, false)).unwrap();
    let missing = JobCommand::NetworkStatus {
        plan_id: Uuid::new_v4().to_string(),
        plan: Box::new(plan.clone()),
        side: TunnelEndpointSide::Left,
        runtime_adapter: None,
    };
    assert_eq!(
        validate_job_command(&missing).unwrap_err().code,
        "network_status_adapter_snapshot_required"
    );

    let bound = JobCommand::NetworkStatus {
        plan_id: Uuid::new_v4().to_string(),
        plan: Box::new(plan),
        side: TunnelEndpointSide::Left,
        runtime_adapter: Some(runtime_adapter(LEFT_RUNTIME_ADAPTER)),
    };
    validate_job_command(&bound).unwrap();
}

#[test]
fn network_speed_test_accepts_duration_bounded_unlimited_transfer() {
    let plan = plan_tunnel(&test_plan_input(RuntimeTunnelManager::AgentBuiltin, false)).unwrap();
    let command = JobCommand::NetworkSpeedTest {
        plan_id: Uuid::new_v4().to_string(),
        plan: Box::new(plan),
        server_side: TunnelEndpointSide::Left,
        duration_secs: 10,
        max_bytes: 0,
        rate_limit_kbps: 0,
        port: 5201,
        connect_timeout_ms: 5_000,
    };

    validate_job_command(&command).unwrap();
}

#[test]
fn network_status_side_must_match_the_only_dispatch_target() {
    let plan = plan_tunnel(&test_plan_input(RuntimeTunnelManager::AgentBuiltin, false)).unwrap();
    let command = JobCommand::NetworkStatus {
        plan_id: Uuid::new_v4().to_string(),
        plan: Box::new(plan),
        side: TunnelEndpointSide::Left,
        runtime_adapter: None,
    };
    assert!(vpsman_server_core::validate_network_command_targets(
        &command,
        &["client-a".to_string()]
    )
    .is_ok());
    assert!(vpsman_server_core::validate_network_command_targets(
        &command,
        &["client-b".to_string()]
    )
    .is_err());
}

pub(super) fn test_plan_input(manager: RuntimeTunnelManager, ospf: bool) -> TunnelPlanInput {
    let kind = if manager == RuntimeTunnelManager::AgentBuiltin {
        TunnelKind::Gre
    } else {
        TunnelKind::Wireguard
    };
    TunnelPlanInput {
        name: "edge-a-edge-b".to_string(),
        interface_name: "tunab".to_string(),
        kind,
        runtime_control: RuntimeTunnelControl {
            manager,
            left_adapter_definition_id: (manager == RuntimeTunnelManager::CustomAdapter)
                .then(|| LEFT_RUNTIME_ADAPTER.to_string()),
            right_adapter_definition_id: (manager == RuntimeTunnelManager::CustomAdapter)
                .then(|| RIGHT_RUNTIME_ADAPTER.to_string()),
            ..RuntimeTunnelControl::default()
        },
        runtime_topology: Default::default(),
        left_client_id: "client-a".to_string(),
        right_client_id: "client-b".to_string(),
        left_remote_underlay: "203.0.113.1".to_string(),
        right_remote_underlay: "203.0.113.2".to_string(),
        left_local_underlay: None,
        right_local_underlay: None,
        address_pool_cidr: "10.10.0.0/29".to_string(),
        reserved_addresses: Vec::new(),
        ipv4_tunnel: Some(TunnelAddressPair {
            left: "10.10.0.0".to_string(),
            right: "10.10.0.1".to_string(),
            prefix_len: 31,
        }),
        ipv6_address_pool_cidr: None,
        ipv6_tunnel: None,
        latency_primary_family: TunnelAddressFamily::Ipv4,
        bandwidth_mbps: 1234,
        left_mtu: (manager == RuntimeTunnelManager::AgentBuiltin)
            .then(|| vpsman_common::default_tunnel_mtu(kind))
            .flatten(),
        right_mtu: (manager == RuntimeTunnelManager::AgentBuiltin)
            .then(|| vpsman_common::default_tunnel_mtu(kind))
            .flatten(),
        ospf: ospf.then(|| TunnelOspfConfig {
            mode: OspfControlMode::Reviewed,
            planned_latency_ms: 18.0,
            planned_packet_loss_ratio: 0.0,
            preference: 1.0,
            policy: OspfCostPolicy::default(),
            min_cost_delta: 5,
            healthy_windows: 2,
            left_adapter_definition_id: Some(LEFT_ROUTING_ADAPTER.to_string()),
            right_adapter_definition_id: Some(RIGHT_ROUTING_ADAPTER.to_string()),
        }),
    }
}

pub(super) async fn seed_test_plan_adapter_definitions(repo: &Repository, input: &TunnelPlanInput) {
    let mut references = Vec::<(Uuid, &'static str)>::new();
    let mut add_reference = |raw_id: &str, adapter_kind: &'static str| {
        let id = Uuid::parse_str(raw_id).unwrap();
        if !references.contains(&(id, adapter_kind)) {
            references.push((id, adapter_kind));
        }
    };
    if input.runtime_control.manager == RuntimeTunnelManager::CustomAdapter {
        add_reference(
            input
                .runtime_control
                .left_adapter_definition_id
                .as_deref()
                .unwrap(),
            "runtime_tunnel",
        );
        add_reference(
            input
                .runtime_control
                .right_adapter_definition_id
                .as_deref()
                .unwrap(),
            "runtime_tunnel",
        );
    }
    if let Some(ospf) = &input.ospf {
        if let Some(id) = ospf.left_adapter_definition_id.as_deref() {
            add_reference(id, "routing_cost");
        }
        if let Some(id) = ospf.right_adapter_definition_id.as_deref() {
            add_reference(id, "routing_cost");
        }
    }

    let Repository::Postgres(pool) = repo;
    for (id, adapter_kind) in references {
        let command = |verb: &str| {
            serde_json::json!({
                "argv": [format!("/usr/bin/test-{verb}")],
                "max_timeout_secs": 10,
                "max_output_bytes": 16384
            })
        };
        let definition = if adapter_kind == "runtime_tunnel" {
            serde_json::json!({
                "manager": "custom_adapter",
                "contract_version": 1,
                "startup_command": command("start"),
                "cleanup_command": command("cleanup"),
                "status_command": command("status")
            })
        } else {
            serde_json::json!({
                "contract_version": vpsman_common::ROUTING_COST_ADAPTER_CONTRACT_VERSION,
                "status_command": command("status"),
                "update_command": command("update")
            })
        };
        let name = format!("test-{adapter_kind}-{}", id.simple());
        sqlx::query(
            r#"
            INSERT INTO network_adapter_definitions (
                id, adapter_kind, name, definition
            )
            VALUES ($1, $2, $3, $4)
            ON CONFLICT (id) DO NOTHING
            "#,
        )
        .bind(id)
        .bind(adapter_kind)
        .bind(name)
        .bind(sqlx::types::Json(definition))
        .execute(pool)
        .await
        .unwrap();
    }
}

fn runtime_adapter(definition_id: &str) -> RuntimeTunnelAdapterCommands {
    let command = |verb: &str| RuntimeTunnelCommand {
        argv: vec!["/opt/vpsman-adapters/runtime".to_string(), verb.to_string()],
        max_timeout_secs: 10,
        max_output_bytes: 16 * 1024,
    };
    RuntimeTunnelAdapterCommands {
        definition_id: definition_id.to_string(),
        definition_name: "runtime-adapter".to_string(),
        definition_hash: "ab".repeat(32),
        startup: Some(command("start")),
        stop: Some(command("stop")),
        cleanup: None,
        restart: None,
        status: command("status"),
        traffic_limit_apply: None,
    }
}
