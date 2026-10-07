use super::*;
use vpsman_common::PortForwardPoolStrategy;

pub(super) fn render_map(script: &mut String, index: usize, rule: &PortForwardRule) {
    let pool = rule.pool.as_ref().expect("pool rendering");
    let family = if pool.upstreams[0].target_ip.is_ipv4() {
        "ip"
    } else {
        "ip6"
    };
    // Plain `integer` has no width in nft. Infer a 32-bit selector and the
    // address/16-bit service concatenation from expressions instead.
    let _ = write!(script, "  map pf_{index}_pool {{\n    typeof numgen inc mod 1 : {family} daddr . tcp dport\n    flags interval\n    elements = {{ ");
    let mut first = true;
    let mut start = 0_u64;
    for row in pool.upstreams.iter().filter(|row| row.enabled) {
        for port in row.ports.start..=row.ports.end {
            let end = start + u64::from(row.weight) - 1;
            push_element_separator(script, &mut first);
            if start == end {
                let _ = write!(script, "{start}");
            } else {
                let _ = write!(script, "{start}-{end}");
            }
            let _ = write!(script, " : {} . {port}", row.target_ip);
            start = end + 1;
        }
    }
    script.push_str(" }\n  }\n");
}

pub(super) fn render_translation(
    script: &mut String,
    index: usize,
    rule: &PortForwardRule,
    transport: &str,
) {
    let pool = rule.pool.as_ref().expect("pool rendering");
    let total = pool
        .upstreams
        .iter()
        .filter(|row| row.enabled)
        .map(|row| u64::from(row.weight) * u64::from(row.ports.cardinality()))
        .sum::<u64>();
    let family = if pool.upstreams[0].target_ip.is_ipv4() {
        "ip"
    } else {
        "ip6"
    };
    let selector = match pool.strategy {
        PortForwardPoolStrategy::RoundRobin => format!("numgen inc mod {total}"),
        PortForwardPoolStrategy::Random => format!("numgen random mod {total}"),
        PortForwardPoolStrategy::SourceIpHash => {
            // Explicit, rule-stable seed keeps client affinity across unrelated table rebuilds.
            let seed = u32::from_be_bytes(rule.id.as_bytes()[..4].try_into().expect("UUID prefix"));
            format!("jhash {family} saddr mod {total} seed {seed}")
        }
        _ => unreachable!("native pool strategy validated"),
    };
    let _ = writeln!(
        script,
        "    meta l4proto {transport} dnat {family} addr . port to {selector} map @pf_{index}_pool"
    );
}
