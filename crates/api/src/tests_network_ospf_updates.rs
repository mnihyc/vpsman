use chrono::{Duration, Utc};
use tokio::time::{sleep, Duration as TokioDuration};
use uuid::Uuid;
use vpsman_common::{
    plan_tunnel, CommandOutput, OspfControlMode, OspfCostPolicy, OutputStream, TunnelAddressPair,
    TunnelEndpointSide, TunnelKind, TunnelOspfConfig, TunnelPlanInput,
};

use crate::{
    model::{AuthContext, OperatorPreferences, OperatorView, SourceTemplateView},
    repository::Repository,
    MemoryState,
};

const LEFT_ADAPTER: &str = "33333333-3333-4333-8333-333333333333";
const RIGHT_ADAPTER: &str = "44444444-4444-4444-8444-444444444444";

#[tokio::test]
async fn reviewed_update_plan_is_daemon_neutral_and_requires_verified_endpoints() {
    let (repo, plan_id) = seeded_plan(OspfControlMode::Reviewed, 2).await;
    let before = repo.list_network_ospf_update_plans(10).await.unwrap();
    assert_eq!(before[0].status, "needs_adapter_status");
    assert!(!before[0].requires_approval);

    verify_endpoint_costs(&repo, plan_id, 1000).await;
    record_healthy_evidence(&repo, "edge-a-edge-b").await;
    let plans = repo.list_network_ospf_update_plans(10).await.unwrap();
    let update = &plans[0];
    assert_eq!(update.control_mode, "reviewed");
    assert_eq!(
        update.plan_revision,
        repo.get_tunnel_plan(plan_id)
            .await
            .unwrap()
            .unwrap()
            .revision
    );
    assert_eq!(update.mutation_mode, "server_issued_adapter_jobs");
    assert_eq!(
        update.left_adapter_template_name.as_deref(),
        Some("left-routing")
    );
    assert_eq!(
        update.right_adapter_template_name.as_deref(),
        Some("right-routing")
    );
    assert!(update.left_adapter_definition_hash.is_some());
    assert!(update.right_adapter_definition_hash.is_some());
    assert_eq!(update.left_current_ospf_cost, Some(1000));
    assert_eq!(update.right_current_ospf_cost, Some(1000));
    assert_eq!(update.status, "review_required");
    assert!(update.requires_approval);
    assert!(update.privilege_required);
    assert!(update.change_summary.contains("Apply OSPF cost"));
}

#[tokio::test]
async fn automatic_update_plan_waits_for_configured_healthy_windows() {
    let (repo, plan_id) = seeded_plan(OspfControlMode::Automatic, 3).await;
    verify_endpoint_costs(&repo, plan_id, 1000).await;
    record_healthy_evidence(&repo, "edge-a-edge-b").await;
    let waiting = repo.list_network_ospf_update_plans(10).await.unwrap();
    assert_eq!(waiting[0].status, "automatic_waiting_evidence");
    assert!(!waiting[0].requires_approval);

    assert_eq!(waiting[0].evidence.healthy_probe_streak, 1);
    assert_eq!(waiting[0].evidence.required_healthy_probe_streak, 3);

    record_probe(&repo, "edge-a-edge-b", Uuid::new_v4(), 80.0, true).await;
    let degraded = repo.list_network_ospf_update_plans(10).await.unwrap();
    assert_eq!(degraded[0].status, "automatic_waiting_evidence");
    assert_eq!(degraded[0].evidence.healthy_probe_streak, 0);

    for latency in [25.0, 24.0, 23.0] {
        record_probe(&repo, "edge-a-edge-b", Uuid::new_v4(), latency, false).await;
    }
    let ready = repo.list_network_ospf_update_plans(10).await.unwrap();
    assert_eq!(ready[0].status, "automatic_ready");
    assert_eq!(ready[0].evidence.healthy_probe_streak, 3);
    assert!(!ready[0].requires_approval);
    assert!(!ready[0].privilege_required);
}

#[tokio::test]
async fn reviewed_plan_can_apply_an_explicitly_reviewed_baseline_or_degraded_recommendation() {
    let (repo, plan_id) = seeded_plan(OspfControlMode::Reviewed, 2).await;
    verify_endpoint_costs(&repo, plan_id, 1000).await;

    let baseline = repo.list_network_ospf_update_plans(10).await.unwrap();
    assert_eq!(baseline[0].status, "review_planned_baseline");
    assert!(baseline[0].requires_approval);

    record_probe(&repo, "edge-a-edge-b", Uuid::new_v4(), 80.0, true).await;
    let degraded = repo.list_network_ospf_update_plans(10).await.unwrap();
    assert_eq!(degraded[0].status, "review_degraded");
    assert!(degraded[0].requires_approval);
}

#[tokio::test]
async fn reviewed_plan_treats_missing_current_cost_as_an_initial_apply() {
    let (repo, plan_id) = seeded_plan(OspfControlMode::Reviewed, 2).await;
    verify_endpoint_costs_optional(&repo, plan_id, None).await;
    record_healthy_evidence(&repo, "edge-a-edge-b").await;

    let plans = repo.list_network_ospf_update_plans(10).await.unwrap();
    assert_eq!(plans[0].status, "review_required");
    assert!(plans[0].requires_approval);
    assert_eq!(plans[0].left_current_ospf_cost, None);
    assert_eq!(plans[0].right_current_ospf_cost, None);
    assert!(plans[0].change_summary.contains("Initialize OSPF cost"));
}

#[tokio::test]
async fn ospf_recommendations_ignore_observations_outside_the_recent_window() {
    let (repo, plan_id) = seeded_plan(OspfControlMode::Reviewed, 2).await;
    verify_endpoint_costs(&repo, plan_id, 1000).await;
    record_healthy_evidence(&repo, "edge-a-edge-b").await;

    let Repository::Memory(memory) = &repo else {
        unreachable!("test uses memory repository")
    };
    let expired = (Utc::now() - Duration::minutes(11)).to_rfc3339();
    for observation in memory.network_observations.write().await.iter_mut() {
        observation.observed_at = expired.clone();
    }

    let plans = repo.list_network_ospf_update_plans(10).await.unwrap();
    assert_eq!(plans[0].status, "review_planned_baseline");
    assert_eq!(plans[0].confidence, "no_recent_observations");
    assert_eq!(plans[0].evidence.sample_count, 0);
}

async fn seeded_plan(mode: OspfControlMode, healthy_windows: u8) -> (Repository, Uuid) {
    let memory = MemoryState::default();
    memory.source_templates.write().await.extend([
        routing_template(LEFT_ADAPTER, "left-routing"),
        routing_template(RIGHT_ADAPTER, "right-routing"),
    ]);
    let repo = Repository::Memory(memory);
    let input = TunnelPlanInput {
        name: "edge-a-edge-b".to_string(),
        interface_name: "tunab".to_string(),
        kind: TunnelKind::Gre,
        runtime_control: Default::default(),
        runtime_topology: Default::default(),
        left_client_id: "left-a".to_string(),
        right_client_id: "right-b".to_string(),
        left_remote_underlay: "198.51.100.10".to_string(),
        right_remote_underlay: "203.0.113.20".to_string(),
        left_local_underlay: None,
        right_local_underlay: None,
        address_pool_cidr: "10.255.0.0/30".to_string(),
        reserved_addresses: Vec::new(),
        ipv4_tunnel: Some(TunnelAddressPair {
            left: "10.255.0.0".to_string(),
            right: "10.255.0.1".to_string(),
            prefix_len: 31,
        }),
        ipv6_address_pool_cidr: None,
        ipv6_tunnel: None,
        latency_primary_family: Default::default(),
        bandwidth_mbps: 100,
        ospf: Some(TunnelOspfConfig {
            mode,
            planned_latency_ms: 18.0,
            planned_packet_loss_ratio: 0.0,
            preference: 1.0,
            policy: OspfCostPolicy::default(),
            min_cost_delta: 5,
            healthy_windows,
            left_adapter_template_id: LEFT_ADAPTER.to_string(),
            right_adapter_template_id: RIGHT_ADAPTER.to_string(),
        }),
    };
    let plan = plan_tunnel(&input).unwrap();
    let saved = repo
        .record_tunnel_plan(&input, &plan, true, &operator())
        .await
        .unwrap();
    (repo, saved.id)
}

async fn verify_endpoint_costs(repo: &Repository, plan_id: Uuid, cost: u16) {
    verify_endpoint_costs_optional(repo, plan_id, Some(cost)).await;
}

async fn verify_endpoint_costs_optional(repo: &Repository, plan_id: Uuid, cost: Option<u16>) {
    let left_job = Uuid::new_v4();
    let right_job = Uuid::new_v4();
    let revision = repo
        .get_tunnel_plan(plan_id)
        .await
        .unwrap()
        .unwrap()
        .revision;
    repo.stage_tunnel_plan_ospf_jobs(
        plan_id,
        revision,
        None,
        None,
        None,
        left_job,
        right_job,
        &operator(),
    )
    .await
    .unwrap();
    repo.record_tunnel_plan_ospf_job_result(
        plan_id,
        TunnelEndpointSide::Left,
        left_job,
        cost,
        true,
    )
    .await
    .unwrap();
    repo.record_tunnel_plan_ospf_job_result(
        plan_id,
        TunnelEndpointSide::Right,
        right_job,
        cost,
        true,
    )
    .await
    .unwrap();
}

async fn record_healthy_evidence(repo: &Repository, plan_name: &str) {
    record_probe(repo, plan_name, Uuid::new_v4(), 20.0, false).await;
    let job_id = Uuid::new_v4();
    repo.record_network_observations(
        job_id,
        "left-a",
        &[CommandOutput {
            job_id,
            stream: OutputStream::Status,
            data: serde_json::to_vec(&serde_json::json!({
                "type": "network_speed_test",
                "role": "client",
                "plan": plan_name,
                "interface": "tunab",
                "peer_client_id": "right-b",
                "server_address": "10.255.0.1",
                "success": true,
                "bytes": 1048576,
                "throughput_mbps": 80.0
            }))
            .unwrap(),
            exit_code: Some(0),
            done: true,
        }],
    )
    .await
    .unwrap();
    sleep(TokioDuration::from_millis(1)).await;
}

async fn record_probe(
    repo: &Repository,
    plan_name: &str,
    job_id: Uuid,
    latency_ms: f64,
    degraded: bool,
) {
    repo.record_network_observations(
        job_id,
        "left-a",
        &[CommandOutput {
            job_id,
            stream: OutputStream::Status,
            data: serde_json::to_vec(&serde_json::json!({
                "type": "network_probe",
                "plan": plan_name,
                "interface": "tunab",
                "peer_client_id": "right-b",
                "target": "10.255.0.1",
                "parsed": {
                    "healthy": !degraded,
                    "latency_avg_ms": latency_ms,
                    "packet_loss_ratio": if degraded { 0.1 } else { 0.0 }
                }
            }))
            .unwrap(),
            exit_code: Some(0),
            done: true,
        }],
    )
    .await
    .unwrap();
    sleep(TokioDuration::from_millis(1)).await;
}

fn routing_template(id: &str, name: &str) -> SourceTemplateView {
    SourceTemplateView {
        id: Uuid::parse_str(id).unwrap(),
        domain: "routing_cost_adapter".to_string(),
        name: name.to_string(),
        scope: "shared".to_string(),
        built_in: false,
        is_default: false,
        owner_client_id: None,
        description: None,
        definition: serde_json::json!({
            "contract_version": 1,
            "status_command": {
                "argv": ["/opt/routing/status"],
                "max_timeout_secs": 10,
                "max_output_bytes": 16384
            },
            "update_command": {
                "argv": ["/opt/routing/update"],
                "max_timeout_secs": 10,
                "max_output_bytes": 16384
            }
        }),
        assigned_client_count: 0,
        created_at: crate::unix_now().to_string(),
        updated_at: crate::unix_now().to_string(),
    }
}

fn operator() -> AuthContext {
    AuthContext {
        operator: OperatorView {
            id: Uuid::new_v4(),
            username: "test-operator".to_string(),
            role: "admin".to_string(),
            scopes: vec!["*".to_string()],
            preferences: OperatorPreferences::default(),
            totp_enabled: false,
            status: "active".to_string(),
            session_refresh_ttl_secs: crate::DEFAULT_REFRESH_TOKEN_TTL_SECS,
            created_at: crate::unix_now().to_string(),
            disabled_at: None,
            deleted_at: None,
        },
        session_id: Uuid::new_v4(),
    }
}
