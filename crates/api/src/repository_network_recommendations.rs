use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use vpsman_common::{observed_ospf_cost, payload_hash, OspfControlMode};

use crate::{
    model::{
        NetworkObservationView, NetworkOspfRecommendationView, NetworkOspfUpdateEvidenceView,
        NetworkOspfUpdatePlanView, SourceTemplateView, TunnelPlanView,
    },
    repository::Repository,
    repository_network_observations::topology_identity_hash_for_plan,
};

const OSPF_EVIDENCE_WINDOW_MINUTES: i64 = 10;
const OSPF_EVIDENCE_QUERY_LIMIT: i64 = 10_000;
const MAX_RECENT_PROBE_SAMPLES_PER_PLAN: usize = 20;
const MAX_RECENT_SPEED_SAMPLES_PER_PLAN: usize = 10;

impl Repository {
    pub(crate) async fn list_network_ospf_recommendations(
        &self,
        limit: i64,
    ) -> Result<Vec<NetworkOspfRecommendationView>> {
        let plans = self.list_tunnel_plans().await?;
        let observations = self.recent_ospf_observations().await?;
        let mut recommendations = plans
            .iter()
            .filter(|plan| plan.enabled && plan.plan.ospf.is_some())
            .map(|plan| recommend_plan_ospf_cost(plan, &observations).view)
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
        let observations = self.recent_ospf_observations().await?;
        let templates = self
            .list_source_templates(Some("routing_cost_adapter"))
            .await?;
        let mut update_plans = plans
            .iter()
            .filter(|plan| plan.enabled && plan.plan.ospf.is_some())
            .map(|plan| {
                build_ospf_update_plan(
                    plan,
                    recommend_plan_ospf_cost(plan, &observations),
                    &templates,
                )
            })
            .collect::<Result<Vec<_>>>()?;
        update_plans.sort_by(|left, right| {
            update_plan_priority(right)
                .cmp(&update_plan_priority(left))
                .then_with(|| left.plan_name.cmp(&right.plan_name))
        });
        Ok(update_plans.into_iter().take(limit as usize).collect())
    }

    async fn recent_ospf_observations(&self) -> Result<Vec<NetworkObservationView>> {
        let since = (Utc::now() - Duration::minutes(OSPF_EVIDENCE_WINDOW_MINUTES)).timestamp();
        self.list_network_observations_since(since, OSPF_EVIDENCE_QUERY_LIMIT)
            .await
    }
}

fn recommend_plan_ospf_cost(
    plan: &TunnelPlanView,
    observations: &[NetworkObservationView],
) -> OspfRecommendationCandidate {
    let ospf = plan
        .input
        .ospf
        .as_ref()
        .expect("OSPF recommendations only include OSPF-enabled plans");
    let planned_cost = plan
        .recommended_ospf_cost
        .expect("OSPF-enabled plans have a planned cost");
    let topology_identity_hash = topology_identity_hash_for_plan(plan);
    let probe_observations = observations
        .iter()
        .filter(|observation| {
            observation_matches_plan(plan, &topology_identity_hash, observation)
                && observation.kind == "network_probe"
        })
        .take(MAX_RECENT_PROBE_SAMPLES_PER_PLAN)
        .collect::<Vec<_>>();
    let speed_observations = observations
        .iter()
        .filter(|observation| {
            observation_matches_plan(plan, &topology_identity_hash, observation)
                && observation.kind == "network_speed_test"
        })
        .take(MAX_RECENT_SPEED_SAMPLES_PER_PLAN)
        .collect::<Vec<_>>();
    let latency_avg_ms = average_observation_value(&probe_observations, |observation| {
        observation.latency_avg_ms
    });
    let packet_loss_avg_ratio = average_observation_value(&probe_observations, |observation| {
        observation.packet_loss_ratio
    })
    .unwrap_or(0.0);
    let throughput_avg_mbps = average_observation_value(&speed_observations, |observation| {
        observation.throughput_mbps
    });
    let throughput_max_mbps = speed_observations
        .iter()
        .filter_map(|observation| observation.throughput_mbps)
        .reduce(f64::max);
    let sample_count =
        i64::try_from(probe_observations.len() + speed_observations.len()).unwrap_or(i64::MAX);
    let degraded_count = i64::try_from(
        probe_observations
            .iter()
            .chain(speed_observations.iter())
            .filter(|observation| observation.healthy == Some(false))
            .count(),
    )
    .unwrap_or(i64::MAX);
    let latest_observed_at = probe_observations
        .iter()
        .chain(speed_observations.iter())
        .max_by_key(|observation| parse_observed_at(&observation.observed_at))
        .map(|observation| observation.observed_at.clone());
    let healthy_probe_streak = probe_observations
        .iter()
        .take_while(|observation| {
            observation.healthy == Some(true) && observation.latency_avg_ms.is_some()
        })
        .count();

    let (recommended_ospf_cost, effective_bandwidth, confidence, reason) = match latency_avg_ms {
        Some(latency) => {
            let (cost, bandwidth) = observed_ospf_cost(
                ospf.policy,
                plan.input.bandwidth_mbps,
                latency,
                packet_loss_avg_ratio,
                ospf.preference,
                throughput_avg_mbps,
            );
            (
                i32::from(cost),
                bandwidth,
                if throughput_avg_mbps.is_some() {
                    "measured"
                } else {
                    "latency_only"
                },
                if degraded_count > 0 {
                    "recent probe or speed-test evidence includes degraded samples"
                } else {
                    "derived from the recent probe and speed-test evidence window"
                },
            )
        }
        None => (
            planned_cost,
            plan.input.bandwidth_mbps,
            if throughput_avg_mbps.is_some() {
                "throughput_only"
            } else {
                "no_recent_observations"
            },
            if throughput_avg_mbps.is_some() {
                "recent throughput exists, but recent latency evidence is unavailable"
            } else {
                "using the planned cost until recent explicit probe evidence exists"
            },
        ),
    };

    let evidence_summary = ospf_evidence_summary(
        latency_avg_ms,
        latency_avg_ms.map(|_| packet_loss_avg_ratio),
        throughput_avg_mbps,
        throughput_max_mbps,
        sample_count,
        degraded_count,
        latest_observed_at.as_deref(),
        reason,
    );
    let recommendation_id = ospf_recommendation_id(
        plan,
        recommended_ospf_cost,
        &evidence_summary,
        latest_observed_at.as_deref(),
    );

    OspfRecommendationCandidate {
        healthy_probe_streak,
        view: NetworkOspfRecommendationView {
            recommendation_id,
            plan_id: plan.id,
            plan_name: plan.name.clone(),
            interface_name: plan.plan.interface_name.clone(),
            left_client_id: plan.left_client_id.clone(),
            right_client_id: plan.right_client_id.clone(),
            configured_bandwidth_mbps: plan.input.bandwidth_mbps,
            effective_bandwidth_mbps: effective_bandwidth,
            plan_ospf_cost: planned_cost,
            recommended_ospf_cost,
            cost_delta: recommended_ospf_cost - planned_cost,
            latency_avg_ms,
            packet_loss_avg_ratio: latency_avg_ms.map(|_| packet_loss_avg_ratio),
            throughput_avg_mbps,
            throughput_max_mbps,
            sample_count,
            degraded_count,
            latest_observed_at,
            confidence: confidence.to_string(),
            reason: reason.to_string(),
            evidence_summary,
        },
    }
}

fn build_ospf_update_plan(
    plan: &TunnelPlanView,
    recommendation: OspfRecommendationCandidate,
    templates: &[SourceTemplateView],
) -> Result<NetworkOspfUpdatePlanView> {
    let healthy_probe_streak = recommendation.healthy_probe_streak;
    let recommendation = recommendation.view;
    let ospf = plan
        .plan
        .ospf
        .as_ref()
        .expect("OSPF update plans only include OSPF-enabled plans");
    let left_template = adapter_snapshot(templates, &ospf.left_adapter_template_id)?;
    let right_template = adapter_snapshot(templates, &ospf.right_adapter_template_id)?;
    let adapters_ready = left_template.is_some() && right_template.is_some();
    let endpoints_verified =
        plan.left_ospf_status == "verified" && plan.right_ospf_status == "verified";
    let current_costs_complete =
        plan.left_current_ospf_cost.is_some() && plan.right_current_ospf_cost.is_some();
    let maximum_cost_delta = [plan.left_current_ospf_cost, plan.right_current_ospf_cost]
        .into_iter()
        .flatten()
        .map(|current| (recommendation.recommended_ospf_cost - current).abs())
        .max()
        .unwrap_or(0);
    let status = update_plan_status(
        &recommendation,
        adapters_ready,
        endpoints_verified,
        current_costs_complete,
        &plan.ospf_status,
        maximum_cost_delta,
        ospf.mode,
        ospf.min_cost_delta,
        ospf.healthy_windows,
        healthy_probe_streak,
    );
    let change_summary = if !endpoints_verified {
        format!(
            "Check both routing adapters before applying cost {} on {}",
            recommendation.recommended_ospf_cost, recommendation.interface_name
        )
    } else if !current_costs_complete {
        format!(
            "Initialize OSPF cost {} on {} for endpoints without a reported current cost",
            recommendation.recommended_ospf_cost, recommendation.interface_name
        )
    } else if maximum_cost_delta == 0 {
        format!(
            "Both endpoints already report cost {} on {}",
            recommendation.recommended_ospf_cost, recommendation.interface_name
        )
    } else {
        format!(
            "Apply OSPF cost {} on {} to both verified endpoints",
            recommendation.recommended_ospf_cost, recommendation.interface_name
        )
    };
    let mutation_ready = matches!(
        status.as_str(),
        "review_required" | "review_degraded" | "review_planned_baseline" | "automatic_ready"
    );
    let requires_approval = ospf.mode == OspfControlMode::Reviewed && mutation_ready;

    Ok(NetworkOspfUpdatePlanView {
        recommendation_id: recommendation.recommendation_id,
        plan_id: recommendation.plan_id,
        plan_revision: plan.revision,
        plan_name: recommendation.plan_name,
        interface_name: recommendation.interface_name,
        left_client_id: recommendation.left_client_id.clone(),
        right_client_id: recommendation.right_client_id.clone(),
        control_mode: ospf_control_mode(ospf.mode).to_string(),
        left_adapter_template_id: ospf.left_adapter_template_id.clone(),
        right_adapter_template_id: ospf.right_adapter_template_id.clone(),
        left_adapter_template_name: left_template.as_ref().map(|value| value.0.clone()),
        right_adapter_template_name: right_template.as_ref().map(|value| value.0.clone()),
        left_adapter_definition_hash: left_template.map(|value| value.1),
        right_adapter_definition_hash: right_template.map(|value| value.1),
        left_current_ospf_cost: plan.left_current_ospf_cost,
        right_current_ospf_cost: plan.right_current_ospf_cost,
        left_ospf_status: plan.left_ospf_status.clone(),
        right_ospf_status: plan.right_ospf_status.clone(),
        recommended_ospf_cost: recommendation.recommended_ospf_cost,
        maximum_cost_delta,
        status,
        confidence: recommendation.confidence.clone(),
        requires_approval,
        privilege_required: requires_approval,
        mutation_mode: "server_issued_adapter_jobs".to_string(),
        approval_scope: vec![
            format!("client:{}", recommendation.left_client_id),
            format!("client:{}", recommendation.right_client_id),
        ],
        evidence: NetworkOspfUpdateEvidenceView {
            configured_bandwidth_mbps: recommendation.configured_bandwidth_mbps,
            effective_bandwidth_mbps: recommendation.effective_bandwidth_mbps,
            latency_avg_ms: recommendation.latency_avg_ms,
            packet_loss_avg_ratio: recommendation.packet_loss_avg_ratio,
            throughput_avg_mbps: recommendation.throughput_avg_mbps,
            throughput_max_mbps: recommendation.throughput_max_mbps,
            sample_count: recommendation.sample_count,
            degraded_count: recommendation.degraded_count,
            healthy_probe_streak: i64::try_from(healthy_probe_streak).unwrap_or(i64::MAX),
            required_healthy_probe_streak: i64::from(ospf.healthy_windows),
            latest_observed_at: recommendation.latest_observed_at,
            reason: recommendation.reason,
        },
        change_summary,
        evidence_summary: recommendation.evidence_summary,
    })
}

fn adapter_snapshot(
    templates: &[SourceTemplateView],
    template_id: &str,
) -> Result<Option<(String, String)>> {
    let Ok(template_id) = uuid::Uuid::parse_str(template_id) else {
        return Ok(None);
    };
    let Some(template) = templates.iter().find(|template| template.id == template_id) else {
        return Ok(None);
    };
    let definition = serde_json::to_vec(&template.definition)?;
    Ok(Some((template.name.clone(), payload_hash(&definition))))
}

fn ospf_recommendation_id(
    plan: &TunnelPlanView,
    recommended_ospf_cost: i32,
    evidence_summary: &str,
    latest_observed_at: Option<&str>,
) -> String {
    let payload = format!(
        "v2|{}|{:?}|{:?}|{}|{}|{}",
        plan.id,
        plan.left_current_ospf_cost,
        plan.right_current_ospf_cost,
        recommended_ospf_cost,
        latest_observed_at.unwrap_or("none"),
        evidence_summary
    );
    format!("ospf-{}", &payload_hash(payload.as_bytes())[..16])
}

fn ospf_evidence_summary(
    latency_avg_ms: Option<f64>,
    packet_loss_avg_ratio: Option<f64>,
    throughput_avg_mbps: Option<f64>,
    throughput_max_mbps: Option<f64>,
    sample_count: i64,
    degraded_count: i64,
    latest_observed_at: Option<&str>,
    reason: &str,
) -> String {
    let latency = latency_avg_ms
        .map(|value| format!("{value:.1} ms avg"))
        .unwrap_or_else(|| "latency unavailable".to_string());
    let loss = packet_loss_avg_ratio
        .map(|value| format!("{:.2}% loss", value * 100.0))
        .unwrap_or_else(|| "loss unavailable".to_string());
    let throughput = throughput_avg_mbps
        .map(|avg| {
            throughput_max_mbps
                .map(|max| format!("{avg:.1} Mbps avg, {max:.1} Mbps max"))
                .unwrap_or_else(|| format!("{avg:.1} Mbps avg"))
        })
        .unwrap_or_else(|| "throughput unavailable".to_string());
    let observed = latest_observed_at.unwrap_or("no observation time");
    format!("{latency}; {loss}; {throughput}; {sample_count} samples; {degraded_count} degraded; latest {observed}; {reason}")
}

fn update_plan_status(
    recommendation: &NetworkOspfRecommendationView,
    adapters_ready: bool,
    endpoints_verified: bool,
    current_costs_complete: bool,
    current_status: &str,
    maximum_cost_delta: i32,
    mode: OspfControlMode,
    min_cost_delta: u16,
    healthy_windows: u8,
    healthy_probe_streak: usize,
) -> String {
    if current_status == "pending" {
        "in_progress".to_string()
    } else if !adapters_ready {
        "adapter_unavailable".to_string()
    } else if !endpoints_verified {
        "needs_adapter_status".to_string()
    } else if current_costs_complete && maximum_cost_delta == 0 {
        "noop".to_string()
    } else if current_costs_complete && maximum_cost_delta < i32::from(min_cost_delta) {
        "below_minimum_delta".to_string()
    } else if mode == OspfControlMode::Automatic
        && !automatic_evidence_ready(recommendation, healthy_windows, healthy_probe_streak)
    {
        "automatic_waiting_evidence".to_string()
    } else if mode == OspfControlMode::Automatic {
        "automatic_ready".to_string()
    } else if recommendation.confidence == "no_recent_observations" {
        "review_planned_baseline".to_string()
    } else if recommendation.degraded_count > 0 {
        "review_degraded".to_string()
    } else {
        "review_required".to_string()
    }
}

fn automatic_evidence_ready(
    recommendation: &NetworkOspfRecommendationView,
    healthy_windows: u8,
    healthy_probe_streak: usize,
) -> bool {
    if healthy_probe_streak < usize::from(healthy_windows)
        || !matches!(
            recommendation.confidence.as_str(),
            "measured" | "latency_only"
        )
    {
        return false;
    }
    recommendation.latest_observed_at.is_some()
}

fn parse_observed_at(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .or_else(|_| DateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S%.f%#z"))
        .ok()
        .map(|value| value.with_timezone(&Utc))
        .or_else(|| {
            value
                .parse::<i64>()
                .ok()
                .and_then(|seconds| DateTime::from_timestamp(seconds, 0))
        })
}

fn update_plan_priority(plan: &NetworkOspfUpdatePlanView) -> i32 {
    match plan.status.as_str() {
        "adapter_unavailable" => 7,
        "needs_adapter_status" => 6,
        "review_degraded" => 5,
        "review_required" => 4,
        "automatic_ready" => 4,
        "automatic_waiting_evidence" => 3,
        "below_minimum_delta" => 2,
        "review_planned_baseline" => 3,
        "in_progress" => 2,
        _ => 1,
    }
}

fn ospf_control_mode(mode: OspfControlMode) -> &'static str {
    match mode {
        OspfControlMode::Reviewed => "reviewed",
        OspfControlMode::Automatic => "automatic",
    }
}

fn observation_matches_plan(
    plan: &TunnelPlanView,
    topology_identity_hash: &str,
    observation: &NetworkObservationView,
) -> bool {
    observation.plan_id == Some(plan.id)
        && observation.topology_identity_hash.as_deref() == Some(topology_identity_hash)
}

fn average_observation_value<F>(observations: &[&NetworkObservationView], value: F) -> Option<f64>
where
    F: Fn(&NetworkObservationView) -> Option<f64>,
{
    let mut total = 0.0;
    let mut samples = 0_u64;
    for observation in observations {
        let Some(value) = value(observation) else {
            continue;
        };
        total += value;
        samples += 1;
    }
    (samples > 0).then_some(total / samples as f64)
}

struct OspfRecommendationCandidate {
    view: NetworkOspfRecommendationView,
    healthy_probe_streak: usize,
}
