use super::models::{BandwidthMbps, OspfCostPolicy, TunnelObservation};

pub const MIN_TUNNEL_BANDWIDTH_MBPS: BandwidthMbps = 10;
pub const MAX_TUNNEL_BANDWIDTH_MBPS: BandwidthMbps = 10_000;
const BANDWIDTH_REFERENCE_MBPS: f64 = 100.0;

pub fn ospf_cost(policy: OspfCostPolicy, observation: TunnelObservation) -> u16 {
    let default_policy = OspfCostPolicy::default();
    let bandwidth_mbps = observation
        .bandwidth_mbps
        .clamp(MIN_TUNNEL_BANDWIDTH_MBPS, MAX_TUNNEL_BANDWIDTH_MBPS)
        as f64;
    let latency_ms = finite_or(observation.latency_ms, 0.0).max(0.0);
    let packet_loss_ratio = finite_or(observation.packet_loss_ratio, 0.0).clamp(0.0, 1.0);
    let preference = finite_or(observation.preference, 1.0).max(0.1);
    let latency_weight = finite_or(policy.latency_weight, default_policy.latency_weight);
    let loss_weight = finite_or(policy.loss_weight, default_policy.loss_weight);
    let bandwidth_weight = finite_or(policy.bandwidth_weight, default_policy.bandwidth_weight);
    let preference_bias =
        finite_or(policy.preference_bias, default_policy.preference_bias).max(0.0);
    let min_cost = policy.min_cost.min(policy.max_cost);
    let max_cost = policy.max_cost.max(policy.min_cost);
    let bandwidth_penalty = bandwidth_weight * (BANDWIDTH_REFERENCE_MBPS / bandwidth_mbps).sqrt();
    let raw = (latency_ms * latency_weight) + (packet_loss_ratio * loss_weight) + bandwidth_penalty;
    let biased = raw * preference_bias / preference;
    biased.round().clamp(min_cost as f64, max_cost as f64) as u16
}

fn finite_or(value: f64, fallback: f64) -> f64 {
    if value.is_finite() {
        value
    } else {
        fallback
    }
}

pub fn effective_bandwidth_mbps(
    configured: BandwidthMbps,
    observed_mbps: Option<f64>,
) -> BandwidthMbps {
    let configured = configured.clamp(MIN_TUNNEL_BANDWIDTH_MBPS, MAX_TUNNEL_BANDWIDTH_MBPS);
    match observed_mbps {
        Some(observed) if observed.is_finite() && observed > 0.0 => observed
            .round()
            .clamp(MIN_TUNNEL_BANDWIDTH_MBPS as f64, configured as f64)
            as BandwidthMbps,
        _ => configured,
    }
}

pub fn observed_ospf_cost(
    policy: OspfCostPolicy,
    configured_bandwidth_mbps: BandwidthMbps,
    latency_ms: f64,
    packet_loss_ratio: f64,
    preference: f64,
    observed_throughput_mbps: Option<f64>,
) -> (u16, BandwidthMbps) {
    let effective_bandwidth_mbps =
        effective_bandwidth_mbps(configured_bandwidth_mbps, observed_throughput_mbps);
    let cost = ospf_cost(
        policy,
        TunnelObservation {
            latency_ms,
            packet_loss_ratio,
            bandwidth_mbps: effective_bandwidth_mbps,
            preference,
        },
    );
    (cost, effective_bandwidth_mbps)
}
