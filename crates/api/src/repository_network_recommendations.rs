use anyhow::{Context, Result};
use vpsman_common::{
    observed_ospf_cost, render_tunnel_endpoint_config, BandwidthTier, TunnelEndpointSide,
};

use crate::{
    model::{
        NetworkObservationTrendView, NetworkOspfRecommendationView, NetworkOspfUpdateEvidenceView,
        NetworkOspfUpdatePlanView, TunnelPlanView,
    },
    repository::Repository,
};

impl Repository {
    pub(crate) async fn list_network_ospf_recommendations(
        &self,
        limit: i64,
    ) -> Result<Vec<NetworkOspfRecommendationView>> {
        let plans = self.list_tunnel_plans().await?;
        let trends = self.list_network_observation_trends(1_000).await?;
        let mut recommendations = plans
            .iter()
            .map(|plan| recommend_plan_ospf_cost(plan, &trends))
            .collect::<Vec<_>>();
        recommendations.sort_by(|left, right| {
            right
                .latest_observed_at
                .cmp(&left.latest_observed_at)
                .then_with(|| left.plan_name.cmp(&right.plan_name))
        });
        Ok(recommendations.into_iter().take(limit as usize).collect())
    }

    pub(crate) async fn list_network_ospf_update_plans(
        &self,
        limit: i64,
    ) -> Result<Vec<NetworkOspfUpdatePlanView>> {
        let plans = self.list_tunnel_plans().await?;
        let trends = self.list_network_observation_trends(1_000).await?;
        let mut update_plans = plans
            .iter()
            .map(|plan| {
                let recommendation = recommend_plan_ospf_cost(plan, &trends);
                build_ospf_update_plan(plan, recommendation)
            })
            .collect::<Result<Vec<_>>>()?;
        update_plans.sort_by(|left, right| {
            update_plan_priority(right)
                .cmp(&update_plan_priority(left))
                .then_with(|| left.plan_name.cmp(&right.plan_name))
        });
        Ok(update_plans.into_iter().take(limit as usize).collect())
    }
}

fn recommend_plan_ospf_cost(
    plan: &TunnelPlanView,
    trends: &[NetworkObservationTrendView],
) -> NetworkOspfRecommendationView {
    let relevant = trends
        .iter()
        .filter(|trend| trend_matches_plan(plan, trend))
        .collect::<Vec<_>>();
    let probe_trends = relevant
        .iter()
        .copied()
        .filter(|trend| trend.kind == "network_probe")
        .collect::<Vec<_>>();
    let speed_trends = relevant
        .iter()
        .copied()
        .filter(|trend| trend.kind == "network_speed_test")
        .collect::<Vec<_>>();
    let latency_avg_ms = weighted_average(&probe_trends, |trend| trend.latency_avg_ms);
    let packet_loss_avg_ratio =
        weighted_average(&probe_trends, |trend| trend.packet_loss_avg_ratio).unwrap_or(0.0);
    let throughput_avg_mbps = weighted_average(&speed_trends, |trend| trend.throughput_avg_mbps);
    let throughput_max_mbps = speed_trends
        .iter()
        .filter_map(|trend| trend.throughput_max_mbps)
        .reduce(f64::max);
    let sample_count = relevant.iter().map(|trend| trend.sample_count).sum::<i64>();
    let degraded_count = relevant
        .iter()
        .map(|trend| trend.degraded_count)
        .sum::<i64>();
    let latest_observed_at = relevant
        .iter()
        .map(|trend| trend.latest_observed_at.as_str())
        .max()
        .map(ToOwned::to_owned);
    let (recommended_ospf_cost, effective_bandwidth, confidence, reason) = match latency_avg_ms {
        Some(latency) => {
            let (cost, bandwidth) = observed_ospf_cost(
                plan.input.ospf_policy,
                plan.input.bandwidth,
                latency,
                packet_loss_avg_ratio,
                plan.input.preference,
                throughput_avg_mbps,
            );
            (
                cost as i32,
                bandwidth,
                if throughput_avg_mbps.is_some() {
                    "measured"
                } else {
                    "latency_only"
                },
                if degraded_count > 0 {
                    "probe or speed-test trend has degraded samples"
                } else {
                    "derived from persisted probe/speed-test trends"
                },
            )
        }
        None => (
            plan.recommended_ospf_cost,
            plan.input.bandwidth,
            if throughput_avg_mbps.is_some() {
                "throughput_only"
            } else {
                "no_recent_observations"
            },
            if throughput_avg_mbps.is_some() {
                "throughput exists, but no latency probe trend is available for cost recompute"
            } else {
                "using planned OSPF cost until probe/speed-test trends exist"
            },
        ),
    };

    NetworkOspfRecommendationView {
        plan_id: plan.id,
        plan_name: plan.name.clone(),
        interface_name: plan.plan.interface_name.clone(),
        left_client_id: plan.left_client_id.clone(),
        right_client_id: plan.right_client_id.clone(),
        configured_bandwidth: bandwidth_label(plan.input.bandwidth).to_string(),
        effective_bandwidth: bandwidth_label(effective_bandwidth).to_string(),
        plan_ospf_cost: plan.recommended_ospf_cost,
        recommended_ospf_cost,
        cost_delta: recommended_ospf_cost - plan.recommended_ospf_cost,
        latency_avg_ms,
        packet_loss_avg_ratio: latency_avg_ms.map(|_| packet_loss_avg_ratio),
        throughput_avg_mbps,
        throughput_max_mbps,
        sample_count,
        degraded_count,
        latest_observed_at,
        confidence: confidence.to_string(),
        reason: reason.to_string(),
    }
}

fn build_ospf_update_plan(
    plan: &TunnelPlanView,
    recommendation: NetworkOspfRecommendationView,
) -> Result<NetworkOspfUpdatePlanView> {
    let proposed_cost = u16::try_from(recommendation.recommended_ospf_cost)
        .context("recommended OSPF cost is out of range")?;
    let mut proposed_plan = plan.plan.clone();
    proposed_plan.recommended_ospf_cost = proposed_cost;
    let left_endpoint = render_tunnel_endpoint_config(&proposed_plan, TunnelEndpointSide::Left)?;
    let right_endpoint = render_tunnel_endpoint_config(&proposed_plan, TunnelEndpointSide::Right)?;
    let status = update_plan_status(&recommendation);
    let change_summary = if recommendation.cost_delta == 0 {
        format!(
            "No Bird2 cost change proposed for {} on {}",
            recommendation.plan_name, recommendation.interface_name
        )
    } else {
        format!(
            "Change Bird2 OSPF cost on {} from {} to {} for both tunnel endpoints",
            recommendation.interface_name,
            recommendation.plan_ospf_cost,
            recommendation.recommended_ospf_cost
        )
    };

    Ok(NetworkOspfUpdatePlanView {
        plan_id: recommendation.plan_id,
        plan_name: recommendation.plan_name,
        interface_name: recommendation.interface_name,
        left_client_id: recommendation.left_client_id.clone(),
        right_client_id: recommendation.right_client_id.clone(),
        bird2_file: plan.plan.bird2_file.clone(),
        current_ospf_cost: recommendation.plan_ospf_cost,
        recommended_ospf_cost: recommendation.recommended_ospf_cost,
        cost_delta: recommendation.cost_delta,
        status,
        confidence: recommendation.confidence.clone(),
        requires_approval: recommendation.cost_delta != 0,
        privilege_required: recommendation.cost_delta != 0,
        mutation_mode: "reviewed_plan_only".to_string(),
        approval_scope: vec![
            format!("client:{}", recommendation.left_client_id),
            format!("client:{}", recommendation.right_client_id),
        ],
        evidence: NetworkOspfUpdateEvidenceView {
            configured_bandwidth: recommendation.configured_bandwidth,
            effective_bandwidth: recommendation.effective_bandwidth,
            latency_avg_ms: recommendation.latency_avg_ms,
            packet_loss_avg_ratio: recommendation.packet_loss_avg_ratio,
            throughput_avg_mbps: recommendation.throughput_avg_mbps,
            throughput_max_mbps: recommendation.throughput_max_mbps,
            sample_count: recommendation.sample_count,
            degraded_count: recommendation.degraded_count,
            latest_observed_at: recommendation.latest_observed_at,
            reason: recommendation.reason,
        },
        proposed_left_bird2_interface_snippet: left_endpoint.bird2_interface_snippet,
        proposed_right_bird2_interface_snippet: right_endpoint.bird2_interface_snippet,
        change_summary,
    })
}

fn update_plan_status(recommendation: &NetworkOspfRecommendationView) -> String {
    if recommendation.cost_delta == 0 {
        "noop".to_string()
    } else if recommendation.confidence == "no_recent_observations" {
        "needs_observation".to_string()
    } else if recommendation.degraded_count > 0 {
        "review_degraded".to_string()
    } else {
        "review_required".to_string()
    }
}

fn update_plan_priority(plan: &NetworkOspfUpdatePlanView) -> i32 {
    match plan.status.as_str() {
        "review_degraded" => 4,
        "review_required" => 3,
        "needs_observation" => 2,
        _ => 1,
    }
}

fn trend_matches_plan(plan: &TunnelPlanView, trend: &NetworkObservationTrendView) -> bool {
    if trend.plan_name.as_deref() != Some(plan.name.as_str()) {
        return false;
    }
    if let Some(interface_name) = trend.interface_name.as_deref() {
        if interface_name != plan.plan.interface_name {
            return false;
        }
    }
    let client_pair_matches = trend.client_id == plan.left_client_id
        || trend.client_id == plan.right_client_id
        || trend.peer_client_id.as_deref() == Some(plan.left_client_id.as_str())
        || trend.peer_client_id.as_deref() == Some(plan.right_client_id.as_str());
    client_pair_matches
}

fn weighted_average<F>(trends: &[&NetworkObservationTrendView], value: F) -> Option<f64>
where
    F: Fn(&NetworkObservationTrendView) -> Option<f64>,
{
    let mut total = 0.0;
    let mut samples = 0_i64;
    for trend in trends {
        let Some(value) = value(trend) else {
            continue;
        };
        total += value * trend.sample_count.max(1) as f64;
        samples += trend.sample_count.max(1);
    }
    (samples > 0).then_some(total / samples as f64)
}

fn bandwidth_label(tier: BandwidthTier) -> &'static str {
    match tier {
        BandwidthTier::M10 => "10m",
        BandwidthTier::M100 => "100m",
        BandwidthTier::M1000 => "1000m",
    }
}
