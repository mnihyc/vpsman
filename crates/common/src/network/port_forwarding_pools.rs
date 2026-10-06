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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PortForwardPoolCapabilities {
    pub strategies: Vec<PortForwardPoolStrategy>,
    pub protocols: Vec<PortForwardProtocol>,
    pub address_families: Vec<PortForwardAddressFamily>,
    #[serde(default)]
    pub mixed_families: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub backup_strategies: Vec<PortForwardPoolStrategy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_exclusion: Option<PortForwardFailureCapability>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connect_timeout: Option<PortForwardConnectionCapability>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retries: Option<PortForwardConnectionCapability>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PortForwardFailureCapability {
    pub protocols: Vec<PortForwardProtocol>,
    pub linked_timeout: bool,
    #[serde(default = "one_usize")]
    pub min_endpoints: usize,
    pub description: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PortForwardConnectionCapability {
    pub protocols: Vec<PortForwardProtocol>,
    pub description: String,
}

const fn one_usize() -> usize {
    1
}

impl PortForwardPoolCapabilities {
    pub fn native() -> Self {
        Self {
            strategies: vec![
                PortForwardPoolStrategy::RoundRobin,
                PortForwardPoolStrategy::Random,
                PortForwardPoolStrategy::SourceIpHash,
            ],
            protocols: vec![PortForwardProtocol::Tcp, PortForwardProtocol::Udp],
            address_families: vec![
                PortForwardAddressFamily::Ipv4,
                PortForwardAddressFamily::Ipv6,
            ],
            mixed_families: false,
            backup_strategies: Vec::new(),
            failure_exclusion: None,
            connect_timeout: None,
            retries: None,
        }
    }
}

pub fn port_forward_pool_protocol_supported(
    protocols: &[PortForwardProtocol],
    protocol: PortForwardProtocol,
) -> bool {
    match protocol {
        PortForwardProtocol::Both => {
            protocols.contains(&PortForwardProtocol::Tcp)
                && protocols.contains(&PortForwardProtocol::Udp)
        }
        _ => protocols.contains(&protocol),
    }
}

fn pool_check(condition: bool, message: &str) -> Result<(), PortForwardValidationError> {
    if condition {
        Ok(())
    } else {
        Err(PortForwardValidationError::PoolInvalid(message.to_string()))
    }
}

fn unique_values<T: PartialEq>(values: &[T]) -> bool {
    values
        .iter()
        .enumerate()
        .all(|(i, value)| !values[..i].contains(value))
}

pub fn validate_port_forward_pool_capabilities(
    cap: &PortForwardPoolCapabilities,
) -> Result<(), PortForwardValidationError> {
    let unique = unique_values::<PortForwardPoolStrategy>;
    pool_check(
        !cap.strategies.is_empty() && unique(&cap.strategies),
        "pool strategies must be nonempty and unique",
    )?;
    pool_check(
        !cap.protocols.is_empty()
            && unique_values(&cap.protocols)
            && !cap.protocols.contains(&PortForwardProtocol::Both),
        "capability protocols must list unique tcp and/or udp",
    )?;
    pool_check(
        !cap.address_families.is_empty()
            && unique_values(&cap.address_families)
            && !cap
                .address_families
                .contains(&PortForwardAddressFamily::Both),
        "capability families must list unique ipv4 and/or ipv6",
    )?;
    pool_check(
        unique(&cap.backup_strategies)
            && cap
                .backup_strategies
                .iter()
                .all(|value| cap.strategies.contains(value)),
        "backup strategies must be supported pool strategies",
    )?;
    let feature_protocols = |protocols: &[PortForwardProtocol], description: &str| {
        pool_check(
            !protocols.is_empty()
                && unique_values(protocols)
                && protocols.iter().all(|value| {
                    *value != PortForwardProtocol::Both && cap.protocols.contains(value)
                })
                && description.len() <= 4096,
            "invalid pool feature protocols or description",
        )
    };
    if let Some(failure) = &cap.failure_exclusion {
        feature_protocols(&failure.protocols, &failure.description)?;
        pool_check(
            failure.min_endpoints > 0,
            "minimum failure-tracked endpoint count must be positive",
        )?;
    }
    for feature in [&cap.connect_timeout, &cap.retries].into_iter().flatten() {
        feature_protocols(&feature.protocols, &feature.description)?;
    }
    Ok(())
}

pub fn validate_port_forward_pool(
    pool: &PortForwardPool,
    mode: PortForwardMode,
    protocol: PortForwardProtocol,
    cap: &PortForwardPoolCapabilities,
) -> Result<(), PortForwardValidationError> {
    validate_port_forward_pool_capabilities(cap)?;
    pool_check(
        mode != PortForwardMode::Redirect,
        "REDIRECT cannot use an upstream pool",
    )?;
    pool_check(
        cap.strategies.contains(&pool.strategy)
            && port_forward_pool_protocol_supported(&cap.protocols, protocol),
        "pool strategy or protocol is unsupported",
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
        pool_check(
            cap.address_families.contains(&family),
            "upstream address family is unsupported",
        )?;
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
                || cap.backup_strategies.contains(&pool.strategy),
            "pool strategy does not support backups",
        )?;
        if row.enabled {
            total_weight += u64::from(row.weight) * u64::from(row.ports.cardinality());
        }
        if let Some(policy) = &row.failure_policy {
            let failure = cap.failure_exclusion.as_ref().ok_or_else(|| {
                PortForwardValidationError::PoolInvalid("failure exclusion is unsupported".into())
            })?;
            pool_check(
                port_forward_pool_protocol_supported(&failure.protocols, protocol)
                    && endpoint_count >= failure.min_endpoints as u64,
                "failure exclusion is unsupported for this protocol or endpoint count",
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
                    !failure.linked_timeout || window_secs == retry_after_secs,
                    "failure window and exclusion interval must match for this forwarder",
                )?;
            }
        }
    }
    pool_check(
        cap.mixed_families || families.len() == 1,
        "pool cannot mix address families",
    )?;
    pool_check(
        total_weight <= u64::from(u32::MAX),
        "expanded endpoint weights exceed 4294967295",
    )?;
    if let Some(timeout) = pool.connect_timeout_secs {
        pool_check(
            timeout > 0
                && cap.connect_timeout.as_ref().is_some_and(|cap| {
                    port_forward_pool_protocol_supported(&cap.protocols, protocol)
                }),
            "connect timeout must be positive and supported for this protocol",
        )?;
    }
    if let Some(retry) = &pool.retry_policy {
        pool_check(
            cap.retries
                .as_ref()
                .is_some_and(|cap| port_forward_pool_protocol_supported(&cap.protocols, protocol)),
            "connection retry is unsupported for this protocol",
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

/// Version 2 argv commands read this immutable private file for the invocation.
/// Commands/capabilities are not sent back to the adapter as part of its rule.
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
            contract_version: 2,
            client_id: client_id.to_string(),
            config_hash,
            rule,
        }
    }
}

#[cfg(test)]
#[path = "tests_port_forwarding_pools.rs"]
mod tests;
