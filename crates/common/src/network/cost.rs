use super::models::{BandwidthTier, OspfCostPolicy, TunnelObservation};

pub fn ospf_cost(policy: OspfCostPolicy, observation: TunnelObservation) -> u16 {
    let bandwidth_bonus = policy.bandwidth_weight / observation.bandwidth.mbps() as f64;
    let preference = observation.preference.max(0.1);
    let raw = (observation.latency_ms * policy.latency_weight)
        + (observation.packet_loss_ratio.clamp(0.0, 1.0) * policy.loss_weight)
        + bandwidth_bonus;
    let biased = raw * policy.preference_bias / preference;
    biased
        .round()
        .clamp(policy.min_cost as f64, policy.max_cost as f64) as u16
}

pub fn observed_bandwidth_tier(mbps: f64) -> Option<BandwidthTier> {
    if !mbps.is_finite() || mbps <= 0.0 {
        return None;
    }
    if mbps >= 800.0 {
        Some(BandwidthTier::M1000)
    } else if mbps >= 80.0 {
        Some(BandwidthTier::M100)
    } else {
        Some(BandwidthTier::M10)
    }
}

pub fn effective_bandwidth_tier(
    configured: BandwidthTier,
    observed_mbps: Option<f64>,
) -> BandwidthTier {
    match observed_mbps.and_then(observed_bandwidth_tier) {
        Some(observed) if bandwidth_rank(observed) < bandwidth_rank(configured) => observed,
        _ => configured,
    }
}

pub fn observed_ospf_cost(
    policy: OspfCostPolicy,
    configured_bandwidth: BandwidthTier,
    latency_ms: f64,
    packet_loss_ratio: f64,
    preference: f64,
    observed_throughput_mbps: Option<f64>,
) -> (u16, BandwidthTier) {
    let effective_bandwidth =
        effective_bandwidth_tier(configured_bandwidth, observed_throughput_mbps);
    let cost = ospf_cost(
        policy,
        TunnelObservation {
            latency_ms,
            packet_loss_ratio,
            bandwidth: effective_bandwidth,
            preference,
        },
    );
    (cost, effective_bandwidth)
}

fn bandwidth_rank(tier: BandwidthTier) -> u8 {
    match tier {
        BandwidthTier::M10 => 1,
        BandwidthTier::M100 => 2,
        BandwidthTier::M1000 => 3,
    }
}
