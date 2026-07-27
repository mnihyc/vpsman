use std::collections::{HashMap, HashSet};

use anyhow::{ensure, Result};
use chrono::{DateTime, Utc};
use uuid::Uuid;
use vpsman_common::{
    aggregate_topology_probe_state, aggregate_topology_runtime_state,
    is_topology_edge_health_status, is_topology_neighbor_state, is_topology_node_status,
    is_topology_observation_state, is_topology_probe_state, is_topology_runtime_state,
    topology_runtime_state_is_degraded, TunnelEndpointSide, TunnelKind,
};

use crate::{
    model::{
        AgentView, NetworkObservationTrendView, NetworkObservationView, TelemetryTunnelView,
        TunnelPlanView,
    },
    model_topology::{TopologyGraphEdgeView, TopologyGraphNodeView, TopologyGraphView},
    repository::Repository,
    repository_network_observations::{
        summarize_network_observation_trends, topology_identity_hash_for_plan,
    },
};

impl Repository {
    pub(crate) async fn topology_graph(&self, limit: i64) -> Result<TopologyGraphView> {
        let agents = self.list_agents().await?;
        let plans = self.list_tunnel_plans().await?;
        let plan_ids = plans.iter().map(|plan| plan.id).collect::<Vec<_>>();
        let plan_topologies = plans
            .iter()
            .map(|plan| {
                (
                    plan.id,
                    topology_identity_hash_for_plan(plan),
                    plan.left_client_id.clone(),
                    plan.right_client_id.clone(),
                )
            })
            .collect::<Vec<_>>();
        let plan_id_set = plan_ids.iter().copied().collect::<HashSet<_>>();
        let mut endpoint_client_ids = plans
            .iter()
            .flat_map(|plan| [&plan.left_client_id, &plan.right_client_id])
            .cloned()
            .collect::<Vec<_>>();
        endpoint_client_ids.sort();
        endpoint_client_ids.dedup();
        let observations = self
            .list_network_observations_for_topology(&plan_topologies, limit.clamp(1, 24) as usize)
            .await?;
        let trends = summarize_network_observation_trends(&observations);
        let mut telemetry = self
            .list_declared_telemetry_tunnels_for_source_status_clients(&endpoint_client_ids)
            .await?;
        telemetry.retain(|record| {
            record
                .plan_id
                .is_some_and(|plan_id| plan_id_set.contains(&plan_id))
        });
        let recommendations = self
            .list_network_ospf_recommendations_for_plans(&plans)
            .await?;

        let agent_status = agents
            .iter()
            .map(|agent| (agent.id.clone(), agent.status.clone()))
            .collect::<HashMap<_, _>>();
        let node_catalog = seed_topology_nodes(agents);
        let mut nodes = HashMap::new();
        let mut edges = Vec::with_capacity(plans.len());
        let recommendation_by_plan = recommendations
            .iter()
            .map(|recommendation| (recommendation.plan_id, recommendation))
            .collect::<HashMap<_, _>>();

        for plan in plans {
            nodes.entry(plan.left_client_id.clone()).or_insert_with(|| {
                node_catalog
                    .get(&plan.left_client_id)
                    .cloned()
                    .unwrap_or_else(|| synthetic_node(&plan.left_client_id))
            });
            nodes
                .entry(plan.right_client_id.clone())
                .or_insert_with(|| {
                    node_catalog
                        .get(&plan.right_client_id)
                        .cloned()
                        .unwrap_or_else(|| synthetic_node(&plan.right_client_id))
                });

            let topology_identity_hash = topology_identity_hash_for_plan(&plan);
            let summary = summarize_edge_trends(plan.id, &topology_identity_hash, &trends);
            let evidence =
                summarize_edge_observations(plan.id, &topology_identity_hash, &observations);
            let availability = summarize_endpoint_availability(
                &plan.left_client_id,
                &plan.right_client_id,
                &agent_status,
            );
            let left_runtime = summarize_endpoint_runtime(
                &plan,
                TunnelEndpointSide::Left,
                &telemetry,
                &agent_status,
            );
            let right_runtime = summarize_endpoint_runtime(
                &plan,
                TunnelEndpointSide::Right,
                &telemetry,
                &agent_status,
            );
            let recommendation = recommendation_by_plan.get(&plan.id).copied();
            let health = edge_health(
                plan.enabled,
                &left_runtime.state,
                &right_runtime.state,
                summary.degraded_count,
                evidence.runtime_degraded,
            );
            let runtime_state = aggregate_endpoint_runtime_state(
                &evidence.runtime_state,
                &left_runtime.state,
                &right_runtime.state,
                plan.enabled,
            );
            let edge = TopologyGraphEdgeView {
                plan_id: plan.id,
                topology_identity_hash,
                plan_name: plan.name.clone(),
                interface_name: plan.plan.interface_name.clone(),
                kind: tunnel_kind_label(plan.kind),
                left_client_id: plan.left_client_id.clone(),
                right_client_id: plan.right_client_id.clone(),
                enabled: plan.enabled,
                health: health.clone(),
                left_runtime_state: left_runtime.state,
                right_runtime_state: right_runtime.state,
                left_runtime_reason: left_runtime.reason,
                right_runtime_reason: right_runtime.reason,
                left_reachability_state: left_runtime.reachability_state,
                right_reachability_state: right_runtime.reachability_state,
                left_reachability_reason: left_runtime.reachability_reason,
                right_reachability_reason: right_runtime.reachability_reason,
                left_observed_at: left_runtime.observed_at.clone(),
                right_observed_at: right_runtime.observed_at.clone(),
                unavailable_client_ids: availability.unavailable_client_ids,
                availability_reasons: availability.reasons,
                neighbor_state: evidence.neighbor_state,
                probe_state: evidence.probe_state,
                runtime_state,
                runtime_reasons: evidence.runtime_reasons,
                adapter_state: evidence.adapter_state,
                routing_state: plan_routing_state(&plan.ospf_status).to_string(),
                kernel_link_probe_state: evidence.kernel_link_probe_state,
                kernel_neighbor_probe_state: evidence.kernel_neighbor_probe_state,
                kernel_route_probe_state: evidence.kernel_route_probe_state,
                kernel_namespace_covered: evidence.kernel_namespace_covered,
                desired_missing_count: evidence.desired_missing_count,
                stale_present_count: evidence.stale_present_count,
                bandwidth_mbps: plan.plan.bandwidth_mbps,
                recommended_ospf_cost: recommendation
                    .map(|record| record.recommended_ospf_cost)
                    .or(plan.recommended_ospf_cost),
                cost_delta: recommendation.map(|record| record.cost_delta),
                latency_avg_ms: summary.latency_avg_ms,
                latency_series_ms: evidence.latency_series_ms,
                packet_loss_avg_ratio: summary.packet_loss_avg_ratio,
                throughput_avg_mbps: summary.throughput_avg_mbps,
                throughput_max_mbps: summary.throughput_max_mbps,
                sample_count: summary.sample_count,
                degraded_count: summary.degraded_count,
                latest_observed_at: latest_observed_at([
                    summary.latest_observed_at.as_deref(),
                    left_runtime.observed_at.as_deref(),
                    right_runtime.observed_at.as_deref(),
                ]),
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
                .then_with(|| right.health.cmp(&left.health))
                .then_with(|| left.plan_name.cmp(&right.plan_name))
        });
        validate_topology_contract(&nodes, &edges)?;

        Ok(TopologyGraphView {
            nodes,
            edges,
            generated_at: Utc::now().to_rfc3339(),
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
            is_topology_edge_health_status(&edge.health)
                && is_endpoint_runtime_state(&edge.left_runtime_state)
                && is_endpoint_runtime_state(&edge.right_runtime_state)
                && is_endpoint_reachability_state(&edge.left_reachability_state)
                && is_endpoint_reachability_state(&edge.right_reachability_state)
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
    kernel_link_probe_state: String,
    kernel_neighbor_probe_state: String,
    kernel_route_probe_state: String,
    kernel_namespace_covered: bool,
    desired_missing_count: i64,
    stale_present_count: i64,
    runtime_degraded: bool,
}

#[derive(Default)]
struct EndpointAvailabilitySummary {
    unavailable_client_ids: Vec<String>,
    reasons: Vec<String>,
}

struct EndpointRuntimeSummary {
    state: String,
    reason: Option<String>,
    reachability_state: String,
    reachability_reason: Option<String>,
    observed_at: Option<String>,
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
                    healthy_tunnel_count: 0,
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
        healthy_tunnel_count: 0,
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
    if edge.health == "healthy" {
        node.healthy_tunnel_count += 1;
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
    let mut kernel_link_probe_state = "unknown".to_string();
    let mut kernel_neighbor_probe_state = "unknown".to_string();
    let mut kernel_route_probe_state = "unknown".to_string();
    let mut kernel_namespace_covered = false;
    let mut desired_missing_count = 0_i64;
    let mut stale_present_count = 0_i64;
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
        if neighbor_state != "healthy" {
            neighbor_state = match summary
                .and_then(|summary| summary.get("neighbor_probe_state"))
                .and_then(serde_json::Value::as_str)
            {
                Some("success") => "kernel_probe_success".to_string(),
                Some("failed") => "kernel_probe_failed".to_string(),
                Some("skipped") if neighbor_state == "unknown" => "not_probed".to_string(),
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
            || stale_present_count > 0,
        runtime_state,
        runtime_reasons,
        adapter_state,
        kernel_link_probe_state,
        kernel_neighbor_probe_state,
        kernel_route_probe_state,
        kernel_namespace_covered,
        desired_missing_count,
        stale_present_count,
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

fn summarize_endpoint_runtime(
    plan: &TunnelPlanView,
    side: TunnelEndpointSide,
    telemetry: &[TelemetryTunnelView],
    agent_status: &HashMap<String, String>,
) -> EndpointRuntimeSummary {
    if !plan.enabled {
        return EndpointRuntimeSummary {
            state: "disabled".to_string(),
            reason: Some("plan_disabled".to_string()),
            reachability_state: "not_configured".to_string(),
            reachability_reason: Some("plan_disabled".to_string()),
            observed_at: None,
        };
    }
    let (client_id, side_name) = match side {
        TunnelEndpointSide::Left => (&plan.left_client_id, "left"),
        TunnelEndpointSide::Right => (&plan.right_client_id, "right"),
    };
    match agent_status.get(client_id).map(String::as_str) {
        None => {
            return EndpointRuntimeSummary {
                state: "unknown".to_string(),
                reason: Some("endpoint_not_registered".to_string()),
                reachability_state: "unknown".to_string(),
                reachability_reason: Some("endpoint_not_registered".to_string()),
                observed_at: None,
            }
        }
        Some("stale") => {
            return EndpointRuntimeSummary {
                state: "stale".to_string(),
                reason: Some("endpoint_telemetry_stale".to_string()),
                reachability_state: "unknown".to_string(),
                reachability_reason: Some("endpoint_telemetry_stale".to_string()),
                observed_at: latest_endpoint_observed_at(plan.id, client_id, side_name, telemetry),
            }
        }
        Some("offline") => {
            return EndpointRuntimeSummary {
                state: "degraded".to_string(),
                reason: Some("endpoint_offline".to_string()),
                reachability_state: "unknown".to_string(),
                reachability_reason: Some("endpoint_offline".to_string()),
                observed_at: latest_endpoint_observed_at(plan.id, client_id, side_name, telemetry),
            }
        }
        Some("never") => {
            return EndpointRuntimeSummary {
                state: "unknown".to_string(),
                reason: Some("endpoint_never_seen".to_string()),
                reachability_state: "unknown".to_string(),
                reachability_reason: Some("endpoint_never_seen".to_string()),
                observed_at: None,
            }
        }
        Some("online") => {}
        Some(status) => {
            return EndpointRuntimeSummary {
                state: "degraded".to_string(),
                reason: Some(format!("endpoint_not_online:{status}")),
                reachability_state: "unknown".to_string(),
                reachability_reason: Some(format!("endpoint_not_online:{status}")),
                observed_at: latest_endpoint_observed_at(plan.id, client_id, side_name, telemetry),
            }
        }
    }

    let Some(record) = latest_endpoint_record(plan.id, client_id, side_name, telemetry) else {
        return EndpointRuntimeSummary {
            state: "unknown".to_string(),
            reason: Some("declared_endpoint_not_observed".to_string()),
            reachability_state: "unknown".to_string(),
            reachability_reason: Some("declared_endpoint_not_observed".to_string()),
            observed_at: None,
        };
    };
    let observed_at = Some(record.observed_at.clone());
    let (reachability_state, reachability_reason) = endpoint_reachability(record);
    if let Some(status) = record
        .traffic_status
        .as_deref()
        .filter(|status| *status != "ok")
    {
        return EndpointRuntimeSummary {
            state: "degraded".to_string(),
            reason: record
                .traffic_reason
                .clone()
                .or_else(|| Some(format!("traffic_status:{status}"))),
            reachability_state,
            reachability_reason,
            observed_at,
        };
    }
    if let Some(adapter) = record
        .adapter_health
        .as_ref()
        .filter(|adapter| adapter.configured && !adapter.success)
    {
        return EndpointRuntimeSummary {
            state: "degraded".to_string(),
            reason: adapter
                .reason
                .clone()
                .or_else(|| Some(format!("runtime_adapter_status:{}", adapter.status))),
            reachability_state,
            reachability_reason,
            observed_at,
        };
    }
    if record
        .operstate
        .as_deref()
        .is_some_and(|status| matches!(status, "down" | "lowerlayerdown" | "notpresent"))
    {
        return EndpointRuntimeSummary {
            state: "degraded".to_string(),
            reason: Some(format!(
                "interface_operstate:{}",
                record.operstate.as_deref().unwrap_or("unknown")
            )),
            reachability_state,
            reachability_reason,
            observed_at,
        };
    }
    let positive_evidence = record.traffic_status.as_deref() == Some("ok")
        || record
            .adapter_health
            .as_ref()
            .is_some_and(|adapter| adapter.configured && adapter.success)
        || record
            .operstate
            .as_deref()
            .is_some_and(|status| !matches!(status, "down" | "lowerlayerdown" | "notpresent"));
    EndpointRuntimeSummary {
        state: if positive_evidence {
            "healthy"
        } else {
            "unknown"
        }
        .to_string(),
        reason: (!positive_evidence).then(|| "runtime_evidence_incomplete".to_string()),
        reachability_state,
        reachability_reason,
        observed_at,
    }
}

fn endpoint_reachability(record: &TelemetryTunnelView) -> (String, Option<String>) {
    match record.latency_status.as_deref() {
        Some("healthy") => ("reachable".to_string(), None),
        Some("down" | "missed" | "failed") => (
            "probe_failed".to_string(),
            record
                .latency_reason
                .clone()
                .or_else(|| Some("latency_probe_failed".to_string())),
        ),
        Some("disabled" | "unconfigured") => {
            ("not_configured".to_string(), record.latency_reason.clone())
        }
        _ => ("unknown".to_string(), record.latency_reason.clone()),
    }
}

fn latest_endpoint_record<'a>(
    plan_id: Uuid,
    client_id: &str,
    side: &str,
    telemetry: &'a [TelemetryTunnelView],
) -> Option<&'a TelemetryTunnelView> {
    telemetry
        .iter()
        .filter(|record| {
            record.plan_id == Some(plan_id)
                && record.client_id == client_id
                && record.endpoint_side.as_deref() == Some(side)
        })
        .max_by_key(|record| timestamp_seconds(&record.observed_at).unwrap_or(i64::MIN))
}

fn latest_endpoint_observed_at(
    plan_id: Uuid,
    client_id: &str,
    side: &str,
    telemetry: &[TelemetryTunnelView],
) -> Option<String> {
    latest_endpoint_record(plan_id, client_id, side, telemetry)
        .map(|record| record.observed_at.clone())
}

fn aggregate_endpoint_runtime_state(
    detailed_state: &str,
    left_state: &str,
    right_state: &str,
    enabled: bool,
) -> String {
    if !enabled {
        return "not_configured".to_string();
    }
    let endpoint_state = if matches!(left_state, "degraded" | "stale")
        || matches!(right_state, "degraded" | "stale")
    {
        "degraded"
    } else if left_state == "unknown" || right_state == "unknown" {
        "unknown"
    } else {
        "healthy"
    };
    if topology_runtime_state_is_degraded(detailed_state) {
        detailed_state.to_string()
    } else if endpoint_state == "unknown" {
        "unknown".to_string()
    } else {
        aggregate_topology_runtime_state(detailed_state, endpoint_state).to_string()
    }
}

fn is_endpoint_runtime_state(value: &str) -> bool {
    matches!(
        value,
        "disabled" | "unknown" | "stale" | "healthy" | "degraded"
    )
}

fn is_endpoint_reachability_state(value: &str) -> bool {
    matches!(
        value,
        "unknown" | "reachable" | "probe_failed" | "not_configured"
    )
}

fn latest_observed_at<'a>(values: impl IntoIterator<Item = Option<&'a str>>) -> Option<String> {
    values
        .into_iter()
        .flatten()
        .max_by_key(|value| timestamp_seconds(value).unwrap_or(i64::MIN))
        .map(str::to_string)
}

fn timestamp_seconds(value: &str) -> Option<i64> {
    value.parse::<i64>().ok().or_else(|| {
        DateTime::parse_from_rfc3339(value)
            .or_else(|_| DateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S%.f%#z"))
            .ok()
            .map(|value| value.with_timezone(&Utc).timestamp())
    })
}

fn summarize_endpoint_availability(
    left_client_id: &str,
    right_client_id: &str,
    agent_status: &HashMap<String, String>,
) -> EndpointAvailabilitySummary {
    let mut unavailable_client_ids = Vec::new();
    let mut reasons = Vec::new();
    for client_id in [left_client_id, right_client_id] {
        match agent_status.get(client_id).map(String::as_str) {
            Some("online") => {}
            Some(status) => {
                unavailable_client_ids.push(client_id.to_string());
                reasons.push(format!("endpoint_not_online:{client_id}:{status}"));
            }
            None => {
                unavailable_client_ids.push(client_id.to_string());
                reasons.push(format!("endpoint_missing:{client_id}"));
            }
        }
    }
    unavailable_client_ids.sort();
    unavailable_client_ids.dedup();
    reasons.sort();
    reasons.dedup();
    EndpointAvailabilitySummary {
        unavailable_client_ids,
        reasons,
    }
}

fn edge_health(
    enabled: bool,
    left_runtime_state: &str,
    right_runtime_state: &str,
    degraded_count: i64,
    runtime_degraded: bool,
) -> String {
    if !enabled {
        "disabled".to_string()
    } else if degraded_count > 0
        || runtime_degraded
        || matches!(left_runtime_state, "degraded" | "stale")
        || matches!(right_runtime_state, "degraded" | "stale")
    {
        "degraded".to_string()
    } else if left_runtime_state == "healthy" && right_runtime_state == "healthy" {
        "healthy".to_string()
    } else {
        "unknown".to_string()
    }
}

fn plan_routing_state(status: &str) -> &'static str {
    match status {
        "disabled" => "not_configured",
        "verified" => "healthy",
        "pending" => "observed",
        "failed" => "routing_unhealthy",
        "stale" => "drift",
        "partial" => "degraded",
        _ => "unknown",
    }
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
