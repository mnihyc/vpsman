use super::*;
use vpsman_common::{
    PortForwardPool, PortForwardPoolStrategy, PortForwardProtocol, PortForwardUpstream,
    PortForwardUpstreamRole, PortRange,
};

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

fn probe_script() -> Result<String> {
    let mut rules = Vec::new();
    for address in ["192.0.2.1", "2001:db8::1"] {
        for strategy in [
            PortForwardPoolStrategy::RoundRobin,
            PortForwardPoolStrategy::Random,
            PortForwardPoolStrategy::SourceIpHash,
        ] {
            let index = rules.len() as u16;
            rules.push(PortForwardRule {
                id: uuid::Uuid::from_u128(u128::from(index) + 1),
                revision: 1,
                name: format!("pool-probe-{index}"),
                protocol: PortForwardProtocol::Both,
                mode: PortForwardMode::Dnat,
                address_family: None,
                adapter: None,
                target_ip: None,
                mappings: Vec::new(),
                masquerade: true,
                pool: Some(PortForwardPool {
                    incoming: vec![PortRange {
                        start: 64000 + index,
                        end: 64000 + index,
                    }],
                    strategy,
                    upstreams: vec![PortForwardUpstream {
                        id: uuid::Uuid::from_u128(1),
                        target_ip: address.parse()?,
                        target_hostname: None,
                        ports: PortRange {
                            start: 8080,
                            end: 8081,
                        },
                        weight: 2,
                        role: PortForwardUpstreamRole::Primary,
                        enabled: true,
                        failure_policy: None,
                    }],
                    connect_timeout_secs: None,
                    retry_policy: None,
                }),
            });
        }
    }
    let config = AgentPortForwardingConfig {
        schema_version: 3,
        desired_hash: port_forwarding_desired_hash(&rules),
        rules,
        ..Default::default()
    };
    Ok(render_apply_script(&config, false)?.replace(
        OWNED_TABLE_NAME,
        &format!("vpsman_pool_probe_{}", std::process::id()),
    ))
}

pub(super) async fn probe() -> bool {
    let Some(nft) = resolve_nft_binary() else {
        return false;
    };
    let Ok(script) = probe_script() else {
        return false;
    };
    // A failed pool probe hides only pools. Existing native mappings stay supported.
    run_nft_script(
        &nft,
        true,
        script.into_bytes(),
        CommandCancelToken::default(),
    )
    .await
    .is_ok()
}
