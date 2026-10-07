use super::*;

/// An explicit pool is independent of the fixed mapping's incoming/target cardinalities.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PortForwardPool {
    pub incoming: Vec<PortRange>,
    pub strategy: PortForwardPoolStrategy,
    pub upstreams: Vec<PortForwardUpstream>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connect_timeout_secs: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_policy: Option<PortForwardRetryPolicy>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PortForwardPoolStrategy {
    RoundRobin,
    Random,
    SourceIpHash,
    LeastConnections,
    ConsistentSourceIpHash,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PortForwardUpstream {
    pub id: Uuid,
    pub target_ip: IpAddr,
    /// API/editor provenance only; removed when constructing runtime desired state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_hostname: Option<String>,
    pub ports: PortRange,
    pub weight: u32,
    pub role: PortForwardUpstreamRole,
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_policy: Option<PortForwardFailurePolicy>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PortForwardUpstreamRole {
    Primary,
    Backup,
}

/// Failure counters and exclusion are independent for each expanded IP:port in a rule.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum PortForwardFailurePolicy {
    Off,
    Temporary {
        threshold: u32,
        window_secs: u32,
        retry_after_secs: u32,
    },
}

/// Limits apply to one incoming connection, not an endpoint's exclusion period.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum PortForwardRetryPolicy {
    Off,
    ConnectFailure {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_attempts: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        retry_budget_secs: Option<u32>,
    },
}

fn pool_check(condition: bool, message: &str) -> Result<(), PortForwardValidationError> {
    if condition {
        Ok(())
    } else {
        Err(PortForwardValidationError::PoolInvalid(message.to_string()))
    }
}

pub fn validate_port_forward_pool(
    pool: &PortForwardPool,
    mode: PortForwardMode,
    protocol: PortForwardProtocol,
) -> Result<(), PortForwardValidationError> {
    pool_check(
        mode != PortForwardMode::Redirect,
        "REDIRECT cannot use an upstream pool",
    )?;
    pool_check(
        mode != PortForwardMode::Dnat
            || matches!(
                pool.strategy,
                PortForwardPoolStrategy::RoundRobin
                    | PortForwardPoolStrategy::Random
                    | PortForwardPoolStrategy::SourceIpHash
            ),
        "DNAT pools use round robin, random, or source IP hash",
    )?;
    pool_check(
        !pool.incoming.is_empty() && pool.incoming.len() <= MAX_PORT_FORWARD_MAPPINGS,
        "invalid pool incoming range count",
    )?;
    for range in &pool.incoming {
        range.validate()?;
    }
    validate_non_overlapping(&pool.incoming)?;
    pool_check(
        !pool.upstreams.is_empty() && pool.upstreams.len() <= MAX_PORT_FORWARD_MAPPINGS,
        "invalid upstream row count",
    )?;
    pool_check(
        pool.upstreams.iter().any(|row| row.enabled),
        "enable at least one upstream",
    )?;
    pool_check(
        pool.upstreams
            .iter()
            .any(|row| row.role == PortForwardUpstreamRole::Primary),
        "keep at least one primary upstream",
    )?;
    let endpoint_count = pool.upstreams.iter().try_fold(0_u64, |sum, row| {
        row.ports.validate()?;
        Ok::<_, PortForwardValidationError>(sum + u64::from(row.ports.cardinality()))
    })?;
    let mut ids = BTreeSet::new();
    let mut families = Vec::new();
    let mut total_weight = 0_u64;
    for (index, row) in pool.upstreams.iter().enumerate() {
        row.ports.validate()?;
        pool_check(ids.insert(row.id), "upstream IDs must be unique")?;
        pool_check(row.weight > 0, "upstream weight must be positive")?;
        if mode == PortForwardMode::Dnat {
            validate_target_ip(row.target_ip)?;
        } else {
            pool_check(
                !row.target_ip.is_unspecified() && !row.target_ip.is_multicast(),
                "upstream target must be a usable unicast address",
            )?;
        }
        let family = if row.target_ip.is_ipv4() {
            PortForwardAddressFamily::Ipv4
        } else {
            PortForwardAddressFamily::Ipv6
        };
        if !families.contains(&family) {
            families.push(family);
        }
        pool_check(
            !pool.upstreams[..index]
                .iter()
                .any(|other| other.target_ip == row.target_ip && other.ports.overlaps(row.ports)),
            "upstream IP/port ranges overlap",
        )?;
        pool_check(
            row.role != PortForwardUpstreamRole::Backup
                || (mode == PortForwardMode::CustomAdapter
                    && matches!(
                        pool.strategy,
                        PortForwardPoolStrategy::RoundRobin
                            | PortForwardPoolStrategy::LeastConnections
                    )),
            "pool strategy does not support backups",
        )?;
        if row.enabled {
            total_weight += u64::from(row.weight) * u64::from(row.ports.cardinality());
        }
        if let Some(policy) = &row.failure_policy {
            pool_check(
                mode == PortForwardMode::CustomAdapter
                    && protocol == PortForwardProtocol::Tcp
                    && endpoint_count >= 2,
                "failure exclusion requires a custom TCP pool with at least two endpoints",
            )?;
            if let PortForwardFailurePolicy::Temporary {
                threshold,
                window_secs,
                retry_after_secs,
            } = policy
            {
                pool_check(
                    *threshold > 0 && *window_secs > 0 && *retry_after_secs > 0,
                    "failure thresholds and intervals must be positive",
                )?;
                pool_check(
                    window_secs == retry_after_secs,
                    "failure window and exclusion interval must match",
                )?;
            }
        }
    }
    pool_check(
        mode != PortForwardMode::Dnat || families.len() == 1,
        "pool cannot mix address families",
    )?;
    pool_check(
        mode != PortForwardMode::Dnat || total_weight <= u64::from(u32::MAX),
        "expanded endpoint weights exceed 4294967295",
    )?;
    if let Some(timeout) = pool.connect_timeout_secs {
        pool_check(
            timeout > 0
                && mode == PortForwardMode::CustomAdapter
                && protocol == PortForwardProtocol::Tcp,
            "connect timeout must be positive and applies to custom TCP pools",
        )?;
    }
    if let Some(retry) = &pool.retry_policy {
        pool_check(
            mode == PortForwardMode::CustomAdapter && protocol == PortForwardProtocol::Tcp,
            "connection retry applies to custom TCP pools",
        )?;
        if let PortForwardRetryPolicy::ConnectFailure {
            max_attempts,
            retry_budget_secs,
        } = retry
        {
            pool_check(
                max_attempts.is_none_or(|n| n > 0) && retry_budget_secs.is_none_or(|n| n > 0),
                "retry limits must be positive when supplied",
            )?;
        }
    }
    Ok(())
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PortForwardUpstreamObservation {
    pub upstream_id: Uuid,
    pub port: u16,
    pub state: PortForwardUpstreamState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_unix: Option<u64>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PortForwardUpstreamState {
    Eligible,
    Excluded,
    Unknown,
}

/// Forwarding argv commands receive this immutable JSON request as an argument.
/// Commands are not sent back to the adapter as part of its rule.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PortForwardAdapterRequest {
    pub contract_version: u16,
    pub client_id: String,
    pub config_hash: String,
    pub rule: PortForwardRule,
}

impl PortForwardAdapterRequest {
    pub fn new(client_id: &str, rule: &PortForwardRule) -> Self {
        let mut rule = rule.clone();
        rule.adapter = None;
        let config_hash = crate::auth::payload_hash(
            &serde_json::to_vec(&(client_id, &rule)).expect("serializable forwarding rule"),
        );
        Self {
            contract_version: PORT_FORWARD_ADAPTER_CONTRACT_VERSION,
            client_id: client_id.to_string(),
            config_hash,
            rule,
        }
    }
}

#[cfg(test)]
#[path = "tests_port_forwarding_pools.rs"]
mod tests;
