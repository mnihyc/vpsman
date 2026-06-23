use uuid::Uuid;
use vpsman_common::{
    observed_ospf_cost, plan_tunnel, BandwidthTier, CommandOutput, OspfCostPolicy, OutputStream,
    TunnelKind, TunnelPlanInput,
};

use crate::{
    model::{AuthContext, OperatorView},
    repository::Repository,
    MemoryState,
};

#[tokio::test]
async fn builds_reviewed_ospf_update_plan_from_observed_recommendation() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = AuthContext {
        operator: OperatorView {
            id: Uuid::nil(),
            username: "test-operator".to_string(),
            role: "admin".to_string(),
            scopes: vec!["*".to_string()],
            preferences: crate::model::OperatorPreferences::default(),
            totp_enabled: false,
            status: "active".to_string(),
            session_refresh_ttl_secs: crate::DEFAULT_REFRESH_TOKEN_TTL_SECS,
            created_at: crate::unix_now().to_string(),
            disabled_at: None,
            deleted_at: None,
        },
        session_id: Uuid::nil(),
    };
    let input = TunnelPlanInput {
        name: "edge-a-edge-b".to_string(),
        interface_name: "tunab".to_string(),
        kind: TunnelKind::Gre,
        runtime_control: Default::default(),
        runtime_topology: Default::default(),
        left_client_id: "left-a".to_string(),
        right_client_id: "right-b".to_string(),
        left_underlay: "198.51.100.10".to_string(),
        right_underlay: "203.0.113.20".to_string(),
        address_pool_cidr: "10.255.0.0/30".to_string(),
        reserved_addresses: Vec::new(),
        ipv4_tunnel: Some(vpsman_common::TunnelAddressPair {
            left: "10.255.0.0".to_string(),
            right: "10.255.0.1".to_string(),
            prefix_len: 31,
        }),
        ipv6_address_pool_cidr: None,
        ipv6_tunnel: None,
        latency_primary_family: Default::default(),
        bandwidth: BandwidthTier::M100,
        latency_ms: 18.0,
        packet_loss_ratio: 0.0,
        preference: 0.5,
        ospf_policy: OspfCostPolicy::default(),
    };
    let plan = plan_tunnel(&input).unwrap();
    repo.record_tunnel_plan(&input, &plan, true, &operator)
        .await
        .unwrap();
    let job_id = Uuid::new_v4();
    repo.record_network_observations(
        job_id,
        "left-a",
        &[
            CommandOutput {
                job_id,
                stream: OutputStream::Status,
                data: serde_json::to_vec(&serde_json::json!({
                    "type": "network_probe",
                    "plan": "edge-a-edge-b",
                    "interface": "tunab",
                    "peer_client_id": "right-b",
                    "target": "10.255.0.1",
                    "parsed": {
                        "healthy": false,
                        "latency_avg_ms": 80.0,
                        "packet_loss_ratio": 0.05
                    }
                }))
                .unwrap(),
                exit_code: Some(0),
                done: true,
            },
            CommandOutput {
                job_id,
                stream: OutputStream::Status,
                data: serde_json::to_vec(&serde_json::json!({
                    "type": "network_speed_test",
                    "role": "client",
                    "plan": "edge-a-edge-b",
                    "interface": "tunab",
                    "peer_client_id": "right-b",
                    "server_address": "10.255.0.1",
                    "port": 5201,
                    "success": true,
                    "bytes": 1048576,
                    "throughput_mbps": 40.0
                }))
                .unwrap(),
                exit_code: Some(0),
                done: true,
            },
        ],
    )
    .await
    .unwrap();

    let update_plans = repo.list_network_ospf_update_plans(10).await.unwrap();
    let update_plan = update_plans
        .iter()
        .find(|item| item.plan_name == "edge-a-edge-b")
        .unwrap();
    let (expected_cost, _) = observed_ospf_cost(
        OspfCostPolicy::default(),
        BandwidthTier::M100,
        80.0,
        0.05,
        0.5,
        Some(40.0),
    );

    assert_eq!(update_plan.status, "review_degraded");
    assert_eq!(update_plan.mutation_mode, "reviewed_plan_only");
    assert_eq!(
        update_plan.current_ospf_cost,
        plan.recommended_ospf_cost as i32
    );
    assert_eq!(update_plan.recommended_ospf_cost, expected_cost as i32);
    assert_eq!(
        update_plan.cost_delta,
        expected_cost as i32 - plan.recommended_ospf_cost as i32
    );
    assert!(update_plan.requires_approval);
    assert!(update_plan.privilege_required);
    assert_eq!(
        update_plan.approval_scope,
        vec!["client:left-a".to_string(), "client:right-b".to_string()]
    );
    assert_eq!(update_plan.evidence.sample_count, 2);
    assert_eq!(update_plan.evidence.degraded_count, 1);
    assert!(update_plan
        .proposed_left_bird2_interface_snippet
        .contains(&format!("cost {};", expected_cost)));
    assert!(update_plan
        .proposed_right_bird2_interface_snippet
        .contains("right-b -> left-a"));
    assert!(update_plan.change_summary.contains(&format!(
        "from {} to {}",
        plan.recommended_ospf_cost, expected_cost
    )));
}
