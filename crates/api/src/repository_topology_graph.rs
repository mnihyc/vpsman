use std::collections::HashMap;

use anyhow::{ensure, Result};
use uuid::Uuid;
use vpsman_common::{
    aggregate_topology_probe_state, aggregate_topology_runtime_state, is_topology_drift_action,
    is_topology_drift_policy, is_topology_edge_health_status, is_topology_neighbor_state,
    is_topology_node_status, is_topology_observation_state, is_topology_probe_state,
    is_topology_runtime_state, topology_runtime_state_is_degraded, BandwidthTier, TunnelKind,
};

use crate::{
    model::{AgentView, NetworkObservationTrendView, NetworkObservationView},
    model_topology::{TopologyGraphEdgeView, TopologyGraphNodeView, TopologyGraphView},
    repository::Repository,
    repository_network_observations::topology_identity_hash_for_plan,
    unix_now,
};

impl Repository {
    pub(crate) async fn topology_graph(&self, limit: i64) -> Result<TopologyGraphView> {
        let agents = self.list_agents().await?;
        let plans = self.list_tunnel_plans().await?;
        let trends = self.list_network_observation_trends(limit).await?;
        let observations = self
            .list_network_observations(limit.saturating_mul(4).clamp(1, 1000))
            .await?;
        let recommendations = self.list_network_ospf_recommendations(limit).await?;

        let agent_status = agents
            .iter()
            .map(|agent| (agent.id.clone(), agent.status.clone()))
            .collect::<HashMap<_, _>>();
        let mut nodes = seed_topology_nodes(agents);
        let mut edges = Vec::with_capacity(plans.len());
        let recommendation_by_plan = recommendations
            .iter()
            .map(|recommendation| (recommendation.plan_id, recommendation))
            .collect::<HashMap<_, _>>();

        for plan in plans {
            nodes
                .entry(plan.left_client_id.clone())
                .or_insert_with(|| synthetic_node(&plan.left_client_id));
            nodes
                .entry(plan.right_client_id.clone())
                .or_insert_with(|| synthetic_node(&plan.right_client_id));

            let topology_identity_hash = topology_identity_hash_for_plan(&plan);
            let summary = summarize_edge_trends(plan.id, &topology_identity_hash, &trends);
            let evidence =
                summarize_edge_observations(plan.id, &topology_identity_hash, &observations);
            let drift =
                summarize_server_drift(&plan.left_client_id, &plan.right_client_id, &agent_status);
            let recommendation = recommendation_by_plan.get(&plan.id).copied();
            let health = edge_health(
                &plan.status,
                summary.degraded_count,
                summary.sample_count,
                drift.convergence_blocked,
                evidence.runtime_degraded,
            );
            let edge = TopologyGraphEdgeView {
                plan_id: plan.id,
                topology_identity_hash,
                plan_name: plan.name.clone(),
                interface_name: plan.plan.interface_name.clone(),
                kind: tunnel_kind_label(plan.kind),
                left_client_id: plan.left_client_id.clone(),
                right_client_id: plan.right_client_id.clone(),
                left_status: plan.left_status.clone(),
                right_status: plan.right_status.clone(),
                status: plan.status.clone(),
                enabled: plan.enabled,
                health: health.clone(),
                convergence_blocked: drift.convergence_blocked,
                offline_client_ids: drift.offline_client_ids.clone(),
                server_drift_reasons: drift.reasons.clone(),
                topology_drift_policy: topology_drift_policy(
                    &health,
                    drift.convergence_blocked,
                    summary.degraded_count,
                    evidence.import_candidate_count,
                    evidence.runtime_degraded,
                ),
                topology_drift_action: topology_drift_action(
                    &health,
                    drift.convergence_blocked,
                    summary.degraded_count,
                    evidence.import_candidate_count,
                    evidence.runtime_degraded,
                ),
                neighbor_state: evidence.neighbor_state,
                probe_state: evidence.probe_state,
                runtime_state: evidence.runtime_state,
                runtime_reasons: evidence.runtime_reasons,
                adapter_state: evidence.adapter_state,
                routing_state: evidence.routing_state,
                kernel_link_probe_state: evidence.kernel_link_probe_state,
                kernel_neighbor_probe_state: evidence.kernel_neighbor_probe_state,
                kernel_route_probe_state: evidence.kernel_route_probe_state,
                kernel_namespace_covered: evidence.kernel_namespace_covered,
                desired_missing_count: evidence.desired_missing_count,
                stale_present_count: evidence.stale_present_count,
                import_candidate_count: evidence.import_candidate_count,
                bandwidth: bandwidth_label(plan.plan.bandwidth),
                recommended_ospf_cost: recommendation
                    .map(|record| record.recommended_ospf_cost)
                    .unwrap_or(plan.recommended_ospf_cost),
                cost_delta: recommendation.map(|record| record.cost_delta),
                latency_avg_ms: summary.latency_avg_ms,
                latency_series_ms: evidence.latency_series_ms,
                packet_loss_avg_ratio: summary.packet_loss_avg_ratio,
                throughput_avg_mbps: summary.throughput_avg_mbps,
                throughput_max_mbps: summary.throughput_max_mbps,
                sample_count: summary.sample_count,
                degraded_count: summary.degraded_count,
                latest_observed_at: summary.latest_observed_at.clone(),
                last_apply_job_id: plan.last_apply_job_id,
                last_rollback_job_id: plan.last_rollback_job_id,
                left_tunnel_address: plan.plan.left_tunnel_address.clone(),
                right_tunnel_address: plan.plan.right_tunnel_address.clone(),
                ipv4_tunnel: plan.plan.ipv4_tunnel.clone(),
                ipv6_tunnel: plan.plan.ipv6_tunnel.clone(),
                latency_primary_family: format!("{:?}", plan.plan.latency_primary_family)
                    .to_ascii_lowercase(),
            };
            update_node_from_edge(&mut nodes, &edge.left_client_id, &edge);
            update_node_from_edge(&mut nodes, &edge.right_client_id, &edge);
            edges.push(edge);
        }

        let mut nodes = nodes.into_values().collect::<Vec<_>>();
        nodes.sort_by(|left, right| {
            left.display_name
                .cmp(&right.display_name)
                .then_with(|| left.client_id.cmp(&right.client_id))
        });
        edges.sort_by(|left, right| {
            right
                .latest_observed_at
                .cmp(&left.latest_observed_at)
                .then_with(|| right.status.cmp(&left.status))
                .then_with(|| left.plan_name.cmp(&right.plan_name))
        });
        validate_topology_contract(&nodes, &edges)?;

        Ok(TopologyGraphView {
            nodes,
            edges,
            generated_at: unix_now().to_string(),
        })
    }
}

fn validate_topology_contract(
    nodes: &[TopologyGraphNodeView],
    edges: &[TopologyGraphEdgeView],
) -> Result<()> {
    for node in nodes {
        ensure!(
            is_topology_node_status(&node.status),
            "topology node status contract drift: {}",
            node.status
        );
    }
    for edge in edges {
        ensure!(
            vpsman_common::TUNNEL_ENDPOINT_STATUSES.contains(&edge.left_status.as_str())
                && vpsman_common::TUNNEL_ENDPOINT_STATUSES.contains(&edge.right_status.as_str())
                && vpsman_common::TUNNEL_PLAN_STATUSES.contains(&edge.status.as_str()),
            "topology tunnel status contract drift: left={} right={} status={}",
            edge.left_status,
            edge.right_status,
            edge.status
        );
        ensure!(
            is_topology_edge_health_status(&edge.health)
                && is_topology_drift_policy(&edge.topology_drift_policy)
                && is_topology_drift_action(&edge.topology_drift_action)
                && is_topology_neighbor_state(&edge.neighbor_state)
                && is_topology_observation_state(&edge.probe_state)
                && is_topology_runtime_state(&edge.runtime_state)
                && is_topology_runtime_state(&edge.adapter_state)
                && is_topology_runtime_state(&edge.routing_state)
                && is_topology_probe_state(&edge.kernel_link_probe_state)
                && is_topology_probe_state(&edge.kernel_neighbor_probe_state)
                && is_topology_probe_state(&edge.kernel_route_probe_state),
            "topology evidence status contract drift for plan {}",
            edge.plan_id
        );
    }
    Ok(())
}

#[derive(Default)]
struct EdgeTrendSummary {
    sample_count: i64,
    degraded_count: i64,
    latency_avg_ms: Option<f64>,
    packet_loss_avg_ratio: Option<f64>,
    throughput_avg_mbps: Option<f64>,
    throughput_max_mbps: Option<f64>,
    latest_observed_at: Option<String>,
}

#[derive(Default)]
struct EdgeObservationSummary {
    latency_series_ms: Vec<f64>,
    probe_state: String,
    neighbor_state: String,
    runtime_state: String,
    runtime_reasons: Vec<String>,
    adapter_state: String,
    routing_state: String,
    kernel_link_probe_state: String,
    kernel_neighbor_probe_state: String,
    kernel_route_probe_state: String,
    kernel_namespace_covered: bool,
    desired_missing_count: i64,
    stale_present_count: i64,
    import_candidate_count: i64,
    runtime_degraded: bool,
}

#[derive(Default)]
struct ServerDriftSummary {
    convergence_blocked: bool,
    offline_client_ids: Vec<String>,
    reasons: Vec<String>,
}

fn seed_topology_nodes(agents: Vec<AgentView>) -> HashMap<String, TopologyGraphNodeView> {
    agents
        .into_iter()
        .map(|agent| {
            (
                agent.id.clone(),
                TopologyGraphNodeView {
                    client_id: agent.id,
                    display_name: agent.display_name,
                    status: agent.status,
                    tags: agent.tags,
                    tunnel_count: 0,
                    applied_tunnel_count: 0,
                    degraded_tunnel_count: 0,
                    latest_observed_at: None,
                },
            )
        })
        .collect()
}

fn synthetic_node(client_id: &str) -> TopologyGraphNodeView {
    TopologyGraphNodeView {
        client_id: client_id.to_string(),
        display_name: client_id.to_string(),
        status: "unknown".to_string(),
        tags: Vec::new(),
        tunnel_count: 0,
        applied_tunnel_count: 0,
        degraded_tunnel_count: 0,
        latest_observed_at: None,
    }
}

fn update_node_from_edge(
    nodes: &mut HashMap<String, TopologyGraphNodeView>,
    client_id: &str,
    edge: &TopologyGraphEdgeView,
) {
    let Some(node) = nodes.get_mut(client_id) else {
        return;
    };
    node.tunnel_count += 1;
    if matches!(edge.health.as_str(), "healthy" | "applied") {
        node.applied_tunnel_count += 1;
    }
    if edge.health == "degraded" {
        node.degraded_tunnel_count += 1;
    }
    if let Some(latest) = edge.latest_observed_at.as_ref() {
        if node
            .latest_observed_at
            .as_ref()
            .is_none_or(|current| latest > current)
        {
            node.latest_observed_at = Some(latest.clone());
        }
    }
}

fn summarize_edge_trends(
    plan_id: Uuid,
    topology_identity_hash: &str,
    trends: &[NetworkObservationTrendView],
) -> EdgeTrendSummary {
    let matching = trends
        .iter()
        .filter(|trend| {
            trend.plan_id == Some(plan_id)
                && trend.topology_identity_hash.as_deref() == Some(topology_identity_hash)
        })
        .collect::<Vec<_>>();
    let sample_count = matching.iter().map(|trend| trend.sample_count).sum();
    let degraded_count = matching.iter().map(|trend| trend.degraded_count).sum();
    let latest_observed_at = matching
        .iter()
        .map(|trend| trend.latest_observed_at.as_str())
        .max()
        .map(ToString::to_string);
    let probes = matching
        .iter()
        .filter(|trend| trend.kind == "network_probe")
        .copied()
        .collect::<Vec<_>>();
    let speeds = matching
        .iter()
        .filter(|trend| trend.kind == "network_speed_test")
        .copied()
        .collect::<Vec<_>>();

    EdgeTrendSummary {
        sample_count,
        degraded_count,
        latency_avg_ms: weighted_average(&probes, |trend| trend.latency_avg_ms),
        packet_loss_avg_ratio: weighted_average(&probes, |trend| trend.packet_loss_avg_ratio),
        throughput_avg_mbps: weighted_average(&speeds, |trend| trend.throughput_avg_mbps),
        throughput_max_mbps: speeds
            .iter()
            .filter_map(|trend| trend.throughput_max_mbps)
            .reduce(f64::max),
        latest_observed_at,
    }
}

fn summarize_edge_observations(
    plan_id: Uuid,
    topology_identity_hash: &str,
    observations: &[NetworkObservationView],
) -> EdgeObservationSummary {
    let mut latency_rows = observations
        .iter()
        .filter(|observation| {
            observation.plan_id == Some(plan_id)
                && observation.topology_identity_hash.as_deref() == Some(topology_identity_hash)
                && observation.kind == "network_probe"
                && observation.latency_avg_ms.is_some()
        })
        .collect::<Vec<_>>();
    latency_rows.sort_by(|left, right| left.observed_at.cmp(&right.observed_at));
    let latency_series_ms = latency_rows
        .into_iter()
        .rev()
        .take(24)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .filter_map(|observation| observation.latency_avg_ms)
        .collect::<Vec<_>>();

    let matching_probe = observations
        .iter()
        .filter(|observation| {
            observation.plan_id == Some(plan_id)
                && observation.topology_identity_hash.as_deref() == Some(topology_identity_hash)
                && observation.kind == "network_probe"
        })
        .collect::<Vec<_>>();
    let probe_state = if matching_probe.is_empty() {
        "unknown"
    } else if matching_probe
        .iter()
        .any(|observation| observation.healthy == Some(false))
    {
        "degraded"
    } else if matching_probe
        .iter()
        .any(|observation| observation.healthy == Some(true))
    {
        "healthy"
    } else {
        "recorded"
    }
    .to_string();

    let mut neighbor_state = "unknown".to_string();
    let mut runtime_state = "unknown".to_string();
    let mut runtime_reasons = Vec::<String>::new();
    let mut adapter_state = "unknown".to_string();
    let mut routing_state = "unknown".to_string();
    let mut kernel_link_probe_state = "unknown".to_string();
    let mut kernel_neighbor_probe_state = "unknown".to_string();
    let mut kernel_route_probe_state = "unknown".to_string();
    let mut kernel_namespace_covered = false;
    let mut desired_missing_count = 0_i64;
    let mut stale_present_count = 0_i64;
    let mut import_candidate_count = 0_i64;
    for observation in observations.iter().filter(|observation| {
        observation.plan_id == Some(plan_id)
            && observation.topology_identity_hash.as_deref() == Some(topology_identity_hash)
            && observation.kind == "network_status"
    }) {
        let summary = observation
            .metadata
            .get("runtime")
            .and_then(|runtime| runtime.get("summary"));
        if let Some(value) = summary
            .and_then(|summary| summary.get("status"))
            .and_then(serde_json::Value::as_str)
        {
            runtime_state = aggregate_runtime_state(&runtime_state, value).to_string();
        }
        if let Some(values) = summary
            .and_then(|summary| summary.get("reasons"))
            .and_then(serde_json::Value::as_array)
        {
            for value in values.iter().filter_map(serde_json::Value::as_str) {
                if !runtime_reasons.iter().any(|existing| existing == value) {
                    runtime_reasons.push(value.to_string());
                }
            }
        }
        if let Some(value) = summary
            .and_then(|summary| summary.get("adapter_state"))
            .and_then(serde_json::Value::as_str)
        {
            adapter_state = aggregate_runtime_state(&adapter_state, value).to_string();
        }
        if let Some(value) = summary
            .and_then(|summary| summary.get("bird2_state"))
            .and_then(serde_json::Value::as_str)
        {
            routing_state = aggregate_runtime_state(&routing_state, value).to_string();
        }
        if let Some(value) = summary
            .and_then(|summary| summary.get("kernel_link_probe_state"))
            .and_then(serde_json::Value::as_str)
        {
            kernel_link_probe_state =
                aggregate_probe_state(&kernel_link_probe_state, value).to_string();
        }
        if let Some(value) = summary
            .and_then(|summary| summary.get("neighbor_probe_state"))
            .and_then(serde_json::Value::as_str)
        {
            kernel_neighbor_probe_state =
                aggregate_probe_state(&kernel_neighbor_probe_state, value).to_string();
        }
        if let Some(value) = summary
            .and_then(|summary| summary.get("route_probe_state"))
            .and_then(serde_json::Value::as_str)
        {
            kernel_route_probe_state =
                aggregate_probe_state(&kernel_route_probe_state, value).to_string();
        }
        kernel_namespace_covered |= summary
            .and_then(|summary| summary.get("real_kernel_namespace_covered"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        if let Some(count) = summary
            .and_then(|summary| summary.get("desired_missing_count"))
            .and_then(serde_json::Value::as_i64)
        {
            desired_missing_count = desired_missing_count.max(count);
        }
        if let Some(count) = summary
            .and_then(|summary| summary.get("stale_present_count"))
            .and_then(serde_json::Value::as_i64)
        {
            stale_present_count = stale_present_count.max(count);
        }
        if let Some(count) = summary
            .and_then(|summary| summary.get("external_import_candidate_count"))
            .and_then(serde_json::Value::as_i64)
        {
            import_candidate_count = import_candidate_count.max(count);
        }
        if neighbor_state != "healthy" {
            neighbor_state = match (
                summary
                    .and_then(|summary| summary.get("bird2_state"))
                    .and_then(serde_json::Value::as_str),
                summary
                    .and_then(|summary| summary.get("neighbor_probe_state"))
                    .and_then(serde_json::Value::as_str),
            ) {
                (Some("healthy"), _) => "healthy".to_string(),
                (_, Some("success")) => "kernel_probe_success".to_string(),
                (_, Some("failed")) => "kernel_probe_failed".to_string(),
                (_, Some("skipped")) if neighbor_state == "unknown" => "not_probed".to_string(),
                _ => neighbor_state,
            };
        }
    }

    EdgeObservationSummary {
        latency_series_ms,
        probe_state,
        neighbor_state,
        runtime_degraded: runtime_state_is_degraded(&runtime_state)
            || desired_missing_count > 0
            || stale_present_count > 0
            || import_candidate_count > 0,
        runtime_state,
        runtime_reasons,
        adapter_state,
        routing_state,
        kernel_link_probe_state,
        kernel_neighbor_probe_state,
        kernel_route_probe_state,
        kernel_namespace_covered,
        desired_missing_count,
        stale_present_count,
        import_candidate_count,
    }
}

fn aggregate_runtime_state(current: &str, next: &str) -> &'static str {
    aggregate_topology_runtime_state(current, next)
}

fn runtime_state_is_degraded(value: &str) -> bool {
    topology_runtime_state_is_degraded(value)
}

fn aggregate_probe_state(current: &str, next: &str) -> &'static str {
    aggregate_topology_probe_state(current, next)
}

fn weighted_average(
    trends: &[&NetworkObservationTrendView],
    value: impl Fn(&NetworkObservationTrendView) -> Option<f64>,
) -> Option<f64> {
    let (weighted, samples) = trends
        .iter()
        .fold((0.0, 0_i64), |(weighted, samples), trend| {
            let Some(value) = value(trend) else {
                return (weighted, samples);
            };
            (
                weighted + value * trend.sample_count as f64,
                samples + trend.sample_count,
            )
        });
    (samples > 0).then_some(weighted / samples as f64)
}

fn summarize_server_drift(
    left_client_id: &str,
    right_client_id: &str,
    agent_status: &HashMap<String, String>,
) -> ServerDriftSummary {
    let mut offline_client_ids = Vec::new();
    let mut reasons = Vec::new();
    for client_id in [left_client_id, right_client_id] {
        match agent_status.get(client_id).map(String::as_str) {
            Some("online") => {}
            Some(status) => {
                offline_client_ids.push(client_id.to_string());
                reasons.push(format!("endpoint_not_online:{client_id}:{status}"));
            }
            None => {
                offline_client_ids.push(client_id.to_string());
                reasons.push(format!("endpoint_missing:{client_id}"));
            }
        }
    }
    offline_client_ids.sort();
    offline_client_ids.dedup();
    reasons.sort();
    reasons.dedup();
    ServerDriftSummary {
        convergence_blocked: !offline_client_ids.is_empty(),
        offline_client_ids,
        reasons,
    }
}

fn edge_health(
    status: &str,
    degraded_count: i64,
    sample_count: i64,
    convergence_blocked: bool,
    runtime_degraded: bool,
) -> String {
    if status.contains("rolled_back") {
        "rolled_back".to_string()
    } else if convergence_blocked || degraded_count > 0 || runtime_degraded {
        "degraded".to_string()
    } else if sample_count > 0 && status.contains("applied") {
        "healthy".to_string()
    } else if status.contains("applied") {
        "applied".to_string()
    } else {
        "planned".to_string()
    }
}

fn topology_drift_policy(
    health: &str,
    convergence_blocked: bool,
    degraded_count: i64,
    import_candidate_count: i64,
    runtime_degraded: bool,
) -> String {
    if convergence_blocked {
        "hold_convergence_until_endpoints_online"
    } else if import_candidate_count > 0 {
        "observe_only_until_import_promoted"
    } else if runtime_degraded {
        "observe_runtime_drift_before_apply"
    } else if degraded_count > 0 || health == "degraded" {
        "observe_and_recommend"
    } else {
        "eligible_for_apply"
    }
    .to_string()
}

fn topology_drift_action(
    health: &str,
    convergence_blocked: bool,
    degraded_count: i64,
    import_candidate_count: i64,
    runtime_degraded: bool,
) -> String {
    if convergence_blocked {
        "wait_for_reconnect"
    } else if import_candidate_count > 0 {
        "promote_observed_first"
    } else if runtime_degraded {
        "inspect_runtime_status"
    } else if degraded_count > 0 || health == "degraded" {
        "inspect_degraded_samples"
    } else {
        "none"
    }
    .to_string()
}

fn tunnel_kind_label(kind: TunnelKind) -> String {
    match kind {
        TunnelKind::Gre => "gre",
        TunnelKind::Ipip => "ipip",
        TunnelKind::Sit => "sit",
        TunnelKind::Fou => "fou",
        TunnelKind::Openvpn => "openvpn",
        TunnelKind::Wireguard => "wireguard",
        TunnelKind::TunTap => "tun_tap",
        TunnelKind::Custom => "custom",
    }
    .to_string()
}

fn bandwidth_label(bandwidth: BandwidthTier) -> String {
    match bandwidth {
        BandwidthTier::M10 => "10m",
        BandwidthTier::M100 => "100m",
        BandwidthTier::M1000 => "1000m",
    }
    .to_string()
}
