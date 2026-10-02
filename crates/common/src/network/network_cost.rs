use super::models::{
    BandwidthMbps, OspfCostPolicy, TunnelEndpointSide, TunnelObservation, TunnelOspfConfig,
};
use num_bigint::BigInt;
use uuid::Uuid;

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

/// Apply the endpoint policy to the existing rounded/clamped dynamic base.
/// Decimal arithmetic preserves operator-entered bucket boundaries (100 * 1.15
/// is 115, not a floating-point approximation just below it). f64's finite
/// decimal representation bounds these integers to a few hundred digits.
pub fn adjusted_ospf_cost(base: u16, config: &TunnelOspfConfig, side: TunnelEndpointSide) -> u16 {
    let (offset, multiplier) = match side {
        TunnelEndpointSide::Left => (config.left_cost_offset, config.left_cost_multiplier),
        TunnelEndpointSide::Right => (config.right_cost_offset, config.right_cost_multiplier),
    };
    let (offset_numerator, offset_denominator) = decimal_ratio(finite_or(offset, 0.0));
    let multiplier = if multiplier.is_finite() && multiplier > 0.0 {
        multiplier
    } else {
        1.0
    };
    let (multiplier_numerator, multiplier_denominator) = decimal_ratio(multiplier);
    let numerator =
        (BigInt::from(base) * &offset_denominator + offset_numerator) * multiplier_numerator;
    let denominator = offset_denominator * multiplier_denominator;
    let min = config.policy.min_cost.min(config.policy.max_cost);
    let max = config.policy.max_cost.max(config.policy.min_cost);
    // All nonpositive adjusted values clamp to min. This also avoids confusing
    // truncating signed integer division with mathematical floor.
    if numerator <= BigInt::from(0) {
        return min;
    }
    let step = BigInt::from(config.cost_floor.max(1));
    let floored = (numerator / (denominator * &step)) * step;
    if floored <= BigInt::from(min) {
        min
    } else if floored >= BigInt::from(max) {
        max
    } else {
        floored.to_string().parse().expect("cost is bounded to u16")
    }
}

fn decimal_ratio(value: f64) -> (BigInt, BigInt) {
    let text = serde_json::to_string(&value).expect("finite decimal serializes");
    let (mantissa, exponent) = text
        .split_once('e')
        .map_or((text.as_str(), 0), |(mantissa, exponent)| {
            (mantissa, exponent.parse::<i32>().expect("decimal exponent"))
        });
    let fractional_digits = mantissa
        .split_once('.')
        .map_or(0, |(_, fraction)| fraction.len() as i32);
    let coefficient =
        BigInt::parse_bytes(mantissa.replace('.', "").as_bytes(), 10).expect("decimal coefficient");
    let exponent = exponent - fractional_digits;
    if exponent >= 0 {
        (
            coefficient * BigInt::from(10).pow(exponent as u32),
            BigInt::from(1),
        )
    } else {
        (coefficient, BigInt::from(10).pow((-exponent) as u32))
    }
}

/// Flooring corrects off-grid reports even below the ordinary minimum delta.
/// Hard min/max bounds are valid exceptions to the grid, not drift to repair.
pub fn ospf_cost_needs_floor_alignment(cost: u16, config: &TunnelOspfConfig) -> bool {
    cost != config.policy.min_cost
        && cost != config.policy.max_cost
        && !cost.is_multiple_of(config.cost_floor.max(1))
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

pub fn routing_cost_update_privilege_payload(
    plan_id: Uuid,
    plan_revision: i64,
    recommendation_id: &str,
    left_current_cost: Option<u16>,
    right_current_cost: Option<u16>,
    left_desired_cost: u16,
    right_desired_cost: u16,
    left_adapter_definition_hash: &str,
    right_adapter_definition_hash: &str,
) -> String {
    format!(
        "v4|{}|{}|{}|{}|{}|{}|{}|{}|{}",
        plan_id,
        plan_revision,
        recommendation_id.trim(),
        left_current_cost.map_or_else(|| "none".to_string(), |value| value.to_string()),
        right_current_cost.map_or_else(|| "none".to_string(), |value| value.to_string()),
        left_desired_cost,
        right_desired_cost,
        left_adapter_definition_hash,
        right_adapter_definition_hash,
    )
}
