use std::collections::{BTreeMap, BTreeSet, HashMap};

use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use uuid::Uuid;
use vpsman_common::{observed_ospf_cost, payload_hash, OspfControlMode};

use crate::{
    model::{
        NetworkAdapterDefinitionView, NetworkObservationView, NetworkOspfRecommendationView,
        NetworkOspfUpdateEvidenceView, NetworkOspfUpdatePlanView, ResolvedOspfCommandSource,
        TunnelPlanView,
    },
    repository::Repository,
    repository_configuration_presets::validate_network_adapter_definition_view,
    repository_network_observations::topology_identity_hash_for_plan,
    util::compare_timestamps_desc,
};

const OSPF_EVIDENCE_WINDOW_MINUTES: i64 = 10;
const MAX_RECENT_PROBE_SAMPLES_PER_PLAN: usize = 20;
const MAX_RECENT_SPEED_SAMPLES_PER_PLAN: usize = 10;

pub(crate) struct AutomaticOspfUpdatePlanBatch {
    pub(crate) updates: Vec<NetworkOspfUpdatePlanView>,
    pub(crate) failures: Vec<AutomaticOspfUpdatePlanFailure>,
}

pub(crate) struct AutomaticOspfUpdatePlanFailure {
    pub(crate) plan_id: Uuid,
    pub(crate) phase: &'static str,
    pub(crate) error: anyhow::Error,
}

impl Repository {
    pub(crate) async fn list_network_ospf_recommendations(
        &self,
        limit: i64,
    ) -> Result<Vec<NetworkOspfRecommendationView>> {
        let plans = self.list_tunnel_plans().await?;
        let mut recommendations = self
            .list_network_ospf_recommendations_for_plans(&plans)
            .await?;
        recommendations.truncate(limit as usize);
        Ok(recommendations)
    }

    pub(crate) async fn list_network_ospf_recommendations_for_plans(
        &self,
        plans: &[TunnelPlanView],
    ) -> Result<Vec<NetworkOspfRecommendationView>> {
        if plans.is_empty() {
            return Ok(Vec::new());
        }
        let eligible_plans = plans
            .iter()
            .filter(|plan| plan.enabled && plan.plan.ospf.is_some())
            .collect::<Vec<_>>();
        let plan_ids = eligible_plans
            .iter()
            .map(|plan| plan.id)
            .collect::<Vec<_>>();
        let observations_by_plan = self.recent_ospf_observations_for_plans(&plan_ids).await?;
        let mut recommendations = eligible_plans
            .iter()
            .map(|plan| {
                recommend_plan_ospf_cost(
                    plan,
                    observations_by_plan
                        .get(&plan.id)
                        .map(Vec::as_slice)
                        .unwrap_or(&[]),
                )
                .view
            })
            .collect::<Vec<_>>();
        recommendations.sort_by(|left, right| {
            compare_optional_timestamps_desc(
                left.latest_observed_at.as_deref(),
                right.latest_observed_at.as_deref(),
            )
            .then_with(|| left.plan_name.cmp(&right.plan_name))
        });
        Ok(recommendations)
    }

    pub(crate) async fn list_network_ospf_update_plans(
        &self,
        limit: i64,
    ) -> Result<Vec<NetworkOspfUpdatePlanView>> {
        self.list_network_ospf_update_plans_matching(limit).await
    }

    #[cfg(test)]
    pub(crate) async fn list_automatic_network_ospf_update_plans(
        &self,
        limit: i64,
    ) -> Result<Vec<NetworkOspfUpdatePlanView>> {
        let plan_ids = self
            .list_automatic_tunnel_plan_ids_for_controller(limit.clamp(1, 1_000) as usize)
            .await?;
        let AutomaticOspfUpdatePlanBatch { updates, failures } = self
            .list_automatic_network_ospf_update_plan_batch(&plan_ids)
            .await?;
        if let Some(failure) = failures.into_iter().next() {
            return Err(anyhow::anyhow!(
                "automatic OSPF update plan {} failed during {}: {:#}",
                failure.plan_id,
                failure.phase,
                failure.error
            ));
        }
        Ok(updates)
    }

    pub(crate) async fn list_automatic_network_ospf_update_plan_batch(
        &self,
        plan_ids: &[Uuid],
    ) -> Result<AutomaticOspfUpdatePlanBatch> {
        let mut failures = Vec::new();
        let plans = self
            .tunnel_plan_record_attempts(plan_ids)
            .await?
            .into_iter()
            .filter_map(|attempt| match attempt.plan {
                Ok(plan)
                    if plan.enabled
                        && plan
                            .plan
                            .ospf
                            .as_ref()
                            .is_some_and(|ospf| ospf.mode == OspfControlMode::Automatic) =>
                {
                    Some(plan)
                }
                Ok(_) => None,
                Err(error) => {
                    failures.push(AutomaticOspfUpdatePlanFailure {
                        plan_id: attempt.plan_id,
                        phase: "automatic_plan_decode",
                        error,
                    });
                    None
                }
            })
            .collect::<Vec<_>>();
        let plan_ids = plans.iter().map(|plan| plan.id).collect::<Vec<_>>();
        let observations_by_plan = self.recent_ospf_observations_for_plans(&plan_ids).await?;
        let adapters = self
            .list_network_adapter_definitions(Some("routing_cost"))
            .await?;
        let fallback_sources = self
            .effective_ospf_command_sources_for_clients(&ospf_fallback_client_ids(plans.iter()))
            .await?;
        let mut updates = Vec::with_capacity(plans.len());
        for plan in &plans {
            match build_ospf_update_plan(
                plan,
                recommend_plan_ospf_cost(
                    plan,
                    observations_by_plan
                        .get(&plan.id)
                        .map(Vec::as_slice)
                        .unwrap_or(&[]),
                ),
                &adapters,
                &fallback_sources,
            ) {
                Ok(update) => updates.push(update),
                Err(error) => failures.push(AutomaticOspfUpdatePlanFailure {
                    plan_id: plan.id,
                    phase: "automatic_update_plan_build",
                    error,
                }),
            }
        }
        updates.sort_by(|left, right| {
            update_plan_priority(right)
                .cmp(&update_plan_priority(left))
                .then_with(|| left.plan_name.cmp(&right.plan_name))
        });
        Ok(AutomaticOspfUpdatePlanBatch { updates, failures })
    }

    async fn list_network_ospf_update_plans_matching(
        &self,
        limit: i64,
    ) -> Result<Vec<NetworkOspfUpdatePlanView>> {
        let plans = self.list_tunnel_plans().await?;
        let eligible_plans = plans
            .iter()
            .filter(|plan| plan.enabled && plan.plan.ospf.is_some())
            .collect::<Vec<_>>();
        let plan_ids = eligible_plans
            .iter()
            .map(|plan| plan.id)
            .collect::<Vec<_>>();
        let observations_by_plan = self.recent_ospf_observations_for_plans(&plan_ids).await?;
        let adapters = self
            .list_network_adapter_definitions(Some("routing_cost"))
            .await?;
        let fallback_sources = self
            .effective_ospf_command_sources_for_clients(&ospf_fallback_client_ids(
                eligible_plans.iter().copied(),
            ))
            .await?;
        let mut update_plans = eligible_plans
            .iter()
            .map(|plan| {
                build_ospf_update_plan(
                    plan,
                    recommend_plan_ospf_cost(
                        plan,
                        observations_by_plan
                            .get(&plan.id)
                            .map(Vec::as_slice)
                            .unwrap_or(&[]),
                    ),
                    &adapters,
                    &fallback_sources,
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

    pub(crate) async fn network_ospf_update_plan_by_id(
        &self,
        plan_id: uuid::Uuid,
    ) -> Result<Option<NetworkOspfUpdatePlanView>> {
        let Some(plan) = self.get_tunnel_plan(plan_id).await? else {
            return Ok(None);
        };
        if !plan.enabled || plan.plan.ospf.is_none() {
            return Ok(None);
        }
        let observations_by_plan = self.recent_ospf_observations_for_plans(&[plan_id]).await?;
        let adapters = self
            .list_network_adapter_definitions(Some("routing_cost"))
            .await?;
        let fallback_sources = self
            .effective_ospf_command_sources_for_clients(&ospf_fallback_client_ids(std::iter::once(
                &plan,
            )))
            .await?;
        build_ospf_update_plan(
            &plan,
            recommend_plan_ospf_cost(
                &plan,
                observations_by_plan
                    .get(&plan_id)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]),
            ),
            &adapters,
            &fallback_sources,
        )
        .map(Some)
    }

    async fn recent_ospf_observations_for_plans(
        &self,
        plan_ids: &[uuid::Uuid],
    ) -> Result<HashMap<uuid::Uuid, Vec<NetworkObservationView>>> {
        if plan_ids.is_empty() {
            return Ok(HashMap::new());
        }
        let since = (Utc::now() - Duration::minutes(OSPF_EVIDENCE_WINDOW_MINUTES)).timestamp();
        let observations = self
            .list_network_observations_for_plans_since(
                plan_ids,
                since,
                MAX_RECENT_PROBE_SAMPLES_PER_PLAN,
                MAX_RECENT_SPEED_SAMPLES_PER_PLAN,
            )
            .await?;
        Ok(observations.into_iter().fold(
            HashMap::<uuid::Uuid, Vec<NetworkObservationView>>::new(),
            |mut grouped, observation| {
                if let Some(plan_id) = observation.plan_id {
                    grouped.entry(plan_id).or_default().push(observation);
                }
                grouped
            },
        ))
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
    });
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
            observation.healthy == Some(true)
                && observation.latency_avg_ms.is_some()
                && observation.packet_loss_ratio.is_some()
        })
        .count();

    let (recommended_ospf_cost, effective_bandwidth, confidence, reason) =
        match (latency_avg_ms, packet_loss_avg_ratio) {
        (Some(latency), Some(packet_loss)) => {
            let (cost, bandwidth) = observed_ospf_cost(
                ospf.policy,
                plan.input.bandwidth_mbps,
                latency,
                packet_loss,
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
        (Some(_), None) => (
            planned_cost,
            plan.input.bandwidth_mbps,
            "incomplete_probe",
            "recent latency exists, but recent packet-loss evidence is unavailable; using the planned cost",
        ),
        (None, Some(_)) => (
            planned_cost,
            plan.input.bandwidth_mbps,
            "incomplete_probe",
            "recent packet-loss evidence exists, but recent latency is unavailable; using the planned cost",
        ),
        (None, None) => (
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
        packet_loss_avg_ratio,
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
            packet_loss_avg_ratio,
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
    adapters: &[NetworkAdapterDefinitionView],
    fallback_sources: &BTreeMap<String, Option<ResolvedOspfCommandSource>>,
) -> Result<NetworkOspfUpdatePlanView> {
    let healthy_probe_streak = recommendation.healthy_probe_streak;
    let recommendation = recommendation.view;
    let ospf = plan
        .plan
        .ospf
        .as_ref()
        .expect("OSPF update plans only include OSPF-enabled plans");
    let left_definition = updater_snapshot(
        adapters,
        fallback_sources,
        &recommendation.left_client_id,
        ospf.left_adapter_definition_id.as_deref(),
    )?;
    let right_definition = updater_snapshot(
        adapters,
        fallback_sources,
        &recommendation.right_client_id,
        ospf.right_adapter_definition_id.as_deref(),
    )?;
    let adapters_ready = left_definition.is_some() && right_definition.is_some();
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
            "Check both endpoint OSPF updaters before applying cost {} on {}",
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
        left_updater_source: left_definition
            .as_ref()
            .map_or_else(|| "unconfigured".to_string(), |value| value.origin.clone()),
        right_updater_source: right_definition
            .as_ref()
            .map_or_else(|| "unconfigured".to_string(), |value| value.origin.clone()),
        left_adapter_definition_id: left_definition.as_ref().map(|value| value.id.clone()),
        right_adapter_definition_id: right_definition.as_ref().map(|value| value.id.clone()),
        left_adapter_definition_name: left_definition.as_ref().map(|value| value.name.clone()),
        right_adapter_definition_name: right_definition.as_ref().map(|value| value.name.clone()),
        left_adapter_definition_hash: left_definition
            .as_ref()
            .map(|value| value.definition_hash.clone()),
        right_adapter_definition_hash: right_definition
            .as_ref()
            .map(|value| value.definition_hash.clone()),
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

struct OspfUpdaterSnapshot {
    origin: String,
    id: String,
    name: String,
    definition_hash: String,
}

fn updater_snapshot(
    adapters: &[NetworkAdapterDefinitionView],
    fallback_sources: &BTreeMap<String, Option<ResolvedOspfCommandSource>>,
    client_id: &str,
    override_definition_id: Option<&str>,
) -> Result<Option<OspfUpdaterSnapshot>> {
    if let Some(definition_id) = override_definition_id {
        let Ok(definition_id) = uuid::Uuid::parse_str(definition_id) else {
            return Ok(None);
        };
        let Some(definition) = adapters
            .iter()
            .find(|definition| definition.id == definition_id)
        else {
            return Ok(None);
        };
        validate_network_adapter_definition_view(definition)?;
        let definition_json = serde_json::to_vec(&definition.definition)?;
        return Ok(Some(OspfUpdaterSnapshot {
            origin: "plan_override".to_string(),
            id: definition.id.to_string(),
            name: definition.name.clone(),
            definition_hash: payload_hash(&definition_json),
        }));
    }
    Ok(fallback_sources
        .get(client_id)
        .and_then(Option::as_ref)
        .map(|source| OspfUpdaterSnapshot {
            origin: source.origin.clone(),
            id: source.id.to_string(),
            name: source.name.clone(),
            definition_hash: source.definition_hash.clone(),
        }))
}

fn ospf_fallback_client_ids<'a>(
    plans: impl IntoIterator<Item = &'a TunnelPlanView>,
) -> Vec<String> {
    let mut clients = BTreeSet::new();
    for plan in plans {
        let Some(ospf) = plan.plan.ospf.as_ref() else {
            continue;
        };
        if ospf.left_adapter_definition_id.is_none() {
            clients.insert(plan.left_client_id.clone());
        }
        if ospf.right_adapter_definition_id.is_none() {
            clients.insert(plan.right_client_id.clone());
        }
    }
    clients.into_iter().collect()
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
        || recommendation.latency_avg_ms.is_none()
        || recommendation.packet_loss_avg_ratio.is_none()
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

fn compare_optional_timestamps_desc(left: Option<&str>, right: Option<&str>) -> std::cmp::Ordering {
    match (left, right) {
        (Some(left), Some(right)) => compare_timestamps_desc(left, right),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    }
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

#[cfg(test)]
mod tests {
    use super::compare_optional_timestamps_desc;
    use std::cmp::Ordering;

    #[test]
    fn recommendation_ordering_handles_mixed_timestamp_formats_and_missing_evidence() {
        assert_eq!(
            compare_optional_timestamps_desc(Some("1770000000"), Some("2026-01-01T00:00:00Z"),),
            Ordering::Less,
        );
        assert_eq!(
            compare_optional_timestamps_desc(Some("1770000000"), None),
            Ordering::Less,
        );
    }
}
