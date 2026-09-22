use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use ipnet::IpNet;

use super::{
    models::{TunnelAddressFamily, TunnelAddressPair, TunnelPlan},
    planner::NetworkPlanError,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TunnelEndpointAllocation {
    pub ipv4_tunnel: Option<TunnelAddressPair>,
    pub ipv6_tunnel: Option<TunnelAddressPair>,
    pub latency_primary_family: TunnelAddressFamily,
}

/// Nonempty preferred endpoints are operator-owned and retained even outside the
/// pool. Empty fields are allocated within the populated peer's subnet, or from
/// the pool when both are empty. Invalid nonempty endpoints are never replaced.
#[derive(Clone, Debug)]
pub struct TunnelEndpointAllocationOptions<'a> {
    pub include_ipv4: bool,
    pub include_ipv6: bool,
    pub ipv4_prefix_len: u8,
    pub ipv6_prefix_len: u8,
    pub preferred_ipv4: Option<&'a TunnelAddressPair>,
    pub preferred_ipv6: Option<&'a TunnelAddressPair>,
}

impl Default for TunnelEndpointAllocationOptions<'_> {
    fn default() -> Self {
        Self {
            include_ipv4: true,
            include_ipv6: false,
            ipv4_prefix_len: 31,
            ipv6_prefix_len: 127,
            preferred_ipv4: None,
            preferred_ipv6: None,
        }
    }
}

/// Compatibility entry point: point-to-point masks remain /31 and /127 unless
/// callers explicitly request another mask through the options entry point.
pub fn allocate_tunnel_endpoints(
    ipv4_pool_cidr: Option<&str>,
    ipv6_pool_cidr: Option<&str>,
    reserved_addresses: &[String],
    include_ipv4: bool,
    include_ipv6: bool,
) -> Result<TunnelEndpointAllocation, NetworkPlanError> {
    allocate_tunnel_endpoints_with_options(
        ipv4_pool_cidr,
        ipv6_pool_cidr,
        reserved_addresses,
        TunnelEndpointAllocationOptions {
            include_ipv4,
            include_ipv6,
            ..Default::default()
        },
    )
}

/// Allocate the first aligned, wholly unoccupied subnet of each requested size.
/// Reservations may be CIDRs (the entire subnet is occupied) or bare host IPs.
/// All caller reservations are checked, including for retained out-of-pool pairs.
/// The repository separately excludes saved plans' interface-local reservations.
pub fn allocate_tunnel_endpoints_with_options(
    ipv4_pool_cidr: Option<&str>,
    ipv6_pool_cidr: Option<&str>,
    reserved_addresses: &[String],
    options: TunnelEndpointAllocationOptions<'_>,
) -> Result<TunnelEndpointAllocation, NetworkPlanError> {
    if !options.include_ipv4 && !options.include_ipv6 {
        return Err(NetworkPlanError::TunnelAddressRequired);
    }
    // Saved-plan reservations already exclude interface-local networks. An
    // explicit caller reservation must still be honored, including link-local.
    let occupied = reserved_addresses
        .iter()
        .map(|value| parse_reservation(value))
        .collect::<Result<Vec<_>, _>>()?;
    let ipv4_tunnel = if options.include_ipv4 {
        Some(resolve_family(
            ipv4_pool_cidr,
            options.ipv4_prefix_len,
            options.preferred_ipv4,
            &occupied,
            true,
        )?)
    } else {
        None
    };
    let ipv6_tunnel = if options.include_ipv6 {
        Some(resolve_family(
            ipv6_pool_cidr,
            options.ipv6_prefix_len,
            options.preferred_ipv6,
            &occupied,
            false,
        )?)
    } else {
        None
    };
    Ok(TunnelEndpointAllocation {
        latency_primary_family: if ipv4_tunnel.is_some() {
            TunnelAddressFamily::Ipv4
        } else {
            TunnelAddressFamily::Ipv6
        },
        ipv4_tunnel,
        ipv6_tunnel,
    })
}

fn resolve_family(
    pool: Option<&str>,
    prefix_len: u8,
    preferred: Option<&TunnelAddressPair>,
    occupied: &[IpNet],
    ipv4: bool,
) -> Result<TunnelAddressPair, NetworkPlanError> {
    let width = if ipv4 { 32 } else { 128 };
    if prefix_len >= width {
        return Err(NetworkPlanError::InvalidTunnelPrefix);
    }
    if let Some(pair) =
        preferred.filter(|pair| !pair.left.trim().is_empty() || !pair.right.trim().is_empty())
    {
        if pair.prefix_len != prefix_len {
            return Err(NetworkPlanError::InvalidPreferredTunnelAddress);
        }
        let pair = fill_preferred_pair(pair, ipv4)?;
        let network = pair_network(&pair, ipv4)
            .map_err(|_| NetworkPlanError::InvalidPreferredTunnelAddress)?;
        if occupied
            .iter()
            .any(|reserved| tunnel_networks_overlap(&network, reserved))
        {
            return Err(NetworkPlanError::TunnelAddressConflict);
        }
        return Ok(pair);
    }
    let pool = pool
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or(NetworkPlanError::AddressPoolRequired)?
        .parse::<IpNet>()
        .map_err(|_| NetworkPlanError::InvalidCidr)?
        .trunc();
    if matches!(pool, IpNet::V4(_)) != ipv4 {
        return Err(NetworkPlanError::InvalidCidr);
    }
    if pool.prefix_len() > prefix_len {
        return Err(NetworkPlanError::AddressPoolTooSmall);
    }
    let intervals = merged_intervals(occupied.iter(), ipv4);
    let (pool_start, pool_end) = network_interval(&pool);
    let host_mask = host_mask(width - prefix_len);
    let start = first_free_subnet(pool_start, pool_end, host_mask, &intervals)?;
    // Traditional IPv4 networks reserve the network/broadcast addresses. /31
    // point-to-point links use both addresses. IPv6 preserves the established
    // first-two-address convention; it has no broadcast address.
    let left = start + u128::from(ipv4 && prefix_len < 31);
    let right = left + 1;
    Ok(TunnelAddressPair {
        left: format_address(left, ipv4),
        right: format_address(right, ipv4),
        prefix_len,
    })
}

fn fill_preferred_pair(
    pair: &TunnelAddressPair,
    ipv4: bool,
) -> Result<TunnelAddressPair, NetworkPlanError> {
    let left_empty = pair.left.trim().is_empty();
    let right_empty = pair.right.trim().is_empty();
    if !left_empty && !right_empty {
        return Ok(pair.clone());
    }
    let anchor = if left_empty { &pair.right } else { &pair.left }
        .parse::<IpAddr>()
        .map_err(|_| NetworkPlanError::InvalidPreferredTunnelAddress)?;
    if anchor.is_ipv4() != ipv4 {
        return Err(NetworkPlanError::InvalidPreferredTunnelAddress);
    }
    let network = IpNet::new(anchor, pair.prefix_len)
        .map_err(|_| NetworkPlanError::InvalidPreferredTunnelAddress)?
        .trunc();
    let (start, _) = network_interval(&network);
    let first = start + u128::from(ipv4 && pair.prefix_len < 31);
    let anchor_value = match anchor {
        IpAddr::V4(address) => u32::from(address) as u128,
        IpAddr::V6(address) => u128::from(address),
    };
    // Every accepted prefix has at least two eligible hosts; at most one is
    // occupied by the retained endpoint. No host-range enumeration is needed.
    let peer = format_address(first + u128::from(first == anchor_value), ipv4);
    Ok(TunnelAddressPair {
        left: if left_empty {
            peer.clone()
        } else {
            pair.left.clone()
        },
        right: if right_empty {
            peer
        } else {
            pair.right.clone()
        },
        prefix_len: pair.prefix_len,
    })
}

fn format_address(value: u128, ipv4: bool) -> String {
    if ipv4 {
        Ipv4Addr::from(value as u32).to_string()
    } else {
        Ipv6Addr::from(value).to_string()
    }
}

fn host_mask(host_bits: u8) -> u128 {
    if host_bits == 128 {
        u128::MAX
    } else {
        (1_u128 << host_bits) - 1
    }
}

/// Intervals are inclusive. Jump beyond each overlapping reservation and align
/// upward to the next subnet, rather than visiting any individual IPs/subnets.
/// Checked addition also represents exhaustion at the end of IPv6's /0 range.
fn first_free_subnet(
    pool_start: u128,
    pool_end: u128,
    host_mask: u128,
    occupied: &[(u128, u128)],
) -> Result<u128, NetworkPlanError> {
    let mut candidate = pool_start;
    for &(start, end) in occupied {
        if end < candidate {
            continue;
        }
        if start > (candidate | host_mask) {
            break;
        }
        candidate = (end | host_mask)
            .checked_add(1)
            .filter(|candidate| *candidate <= pool_end)
            .ok_or(NetworkPlanError::AddressPoolExhausted)?;
    }
    if (candidate | host_mask) <= pool_end {
        Ok(candidate)
    } else {
        Err(NetworkPlanError::AddressPoolExhausted)
    }
}

fn merged_intervals<'a>(
    networks: impl Iterator<Item = &'a IpNet>,
    ipv4: bool,
) -> Vec<(u128, u128)> {
    let mut intervals = networks
        .filter(|network| matches!(network, IpNet::V4(_)) == ipv4)
        .map(network_interval)
        .collect::<Vec<_>>();
    intervals.sort_unstable();
    let mut merged: Vec<(u128, u128)> = Vec::with_capacity(intervals.len());
    for (start, end) in intervals {
        if let Some(last) = merged.last_mut() {
            if start <= last.1.saturating_add(1) {
                last.1 = last.1.max(end);
                continue;
            }
        }
        merged.push((start, end));
    }
    merged
}

fn parse_reservation(value: &str) -> Result<IpNet, NetworkPlanError> {
    let value = value.trim();
    if let Ok(network) = value.parse::<IpNet>() {
        return Ok(network.trunc());
    }
    if let Ok(address) = value.parse::<IpAddr>() {
        return Ok(IpNet::from(address));
    }
    Err(NetworkPlanError::InvalidReservedAddress(value.to_string()))
}

fn pair_network(pair: &TunnelAddressPair, ipv4: bool) -> Result<IpNet, NetworkPlanError> {
    let left = pair
        .left
        .parse::<IpAddr>()
        .map_err(|_| NetworkPlanError::InvalidCidr)?;
    let right = pair
        .right
        .parse::<IpAddr>()
        .map_err(|_| NetworkPlanError::InvalidCidr)?;
    if left == right || left.is_ipv4() != ipv4 || right.is_ipv4() != ipv4 {
        return Err(NetworkPlanError::InvalidCidr);
    }
    let network = IpNet::new(left, pair.prefix_len)
        .map_err(|_| NetworkPlanError::InvalidCidr)?
        .trunc();
    if !network.contains(&right) {
        return Err(NetworkPlanError::InvalidCidr);
    }
    Ok(network)
}

fn network_interval(network: &IpNet) -> (u128, u128) {
    match network {
        IpNet::V4(network) => (
            u32::from(network.network()) as u128,
            u32::from(network.broadcast()) as u128,
        ),
        IpNet::V6(network) => (
            u128::from(network.network()),
            u128::from(network.broadcast()),
        ),
    }
}

fn is_link_local_network(network: &IpNet) -> bool {
    // Only subnets wholly within fe80::/10 are interface-local. For example,
    // fe80::1/0 is the global IPv6 /0 and must not be dropped from reservations.
    matches!(network, IpNet::V6(network)
        if network.prefix_len() >= 10 && network.network().is_unicast_link_local())
}

pub fn tunnel_networks_overlap(left: &IpNet, right: &IpNet) -> bool {
    if matches!(left, IpNet::V4(_)) != matches!(right, IpNet::V4(_)) {
        return false;
    }
    let (left_start, left_end) = network_interval(left);
    let (right_start, right_end) = network_interval(right);
    left_start <= right_end && right_start <= left_end
}

/// Only the main endpoint pairs reserve global allocation space. Additional
/// addresses remain operator-owned interface configuration, not pool claims.
pub fn tunnel_plan_global_networks(plan: &TunnelPlan) -> Result<Vec<IpNet>, NetworkPlanError> {
    let mut networks = Vec::new();
    if let Some(pair) = plan.ipv4_tunnel.as_ref() {
        networks.push(pair_network(pair, true)?);
    }
    if let Some(pair) = plan.ipv6_tunnel.as_ref() {
        networks.push(pair_network(pair, false)?);
    }
    networks.retain(|network| !is_link_local_network(network));
    Ok(networks)
}

#[cfg(test)]
#[path = "tests_allocator.rs"]
mod tests;
