use std::{collections::BTreeSet, net::IpAddr};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

pub const PORT_FORWARDING_SCHEMA_VERSION: u16 = 1;
pub const MAX_PORT_FORWARD_RULES: usize = 512;
pub const MAX_PORT_FORWARD_MAPPINGS: usize = 256;
pub const MAX_PORT_FORWARD_NAME_BYTES: usize = 128;
pub const MAX_PORT_FORWARD_NFT_SCRIPT_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PortForwardProtocol {
    Tcp,
    Udp,
    Both,
}

impl PortForwardProtocol {
    pub fn transports(self) -> &'static [&'static str] {
        match self {
            Self::Tcp => &["tcp"],
            Self::Udp => &["udp"],
            Self::Both => &["tcp", "udp"],
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PortRange {
    pub start: u16,
    pub end: u16,
}

impl PortRange {
    pub fn new(start: u16, end: u16) -> Result<Self, PortForwardValidationError> {
        let value = Self { start, end };
        value.validate()?;
        Ok(value)
    }

    pub fn cardinality(self) -> u32 {
        u32::from(self.end) - u32::from(self.start) + 1
    }

    pub fn is_single(self) -> bool {
        self.start == self.end
    }

    pub fn overlaps(self, other: Self) -> bool {
        self.start <= other.end && other.start <= self.end
    }

    pub fn validate(self) -> Result<(), PortForwardValidationError> {
        if self.start == 0 || self.end == 0 {
            return Err(PortForwardValidationError::PortZero);
        }
        if self.start > self.end {
            return Err(PortForwardValidationError::RangeReversed);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PortForwardMapping {
    pub incoming: PortRange,
    pub target: PortRange,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PortForwardRule {
    pub id: Uuid,
    pub revision: i64,
    pub name: String,
    pub protocol: PortForwardProtocol,
    pub target_ip: IpAddr,
    pub mappings: Vec<PortForwardMapping>,
    #[serde(default = "default_true")]
    pub masquerade: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentPortForwardingConfig {
    #[serde(default = "default_port_forwarding_schema_version")]
    pub schema_version: u16,
    #[serde(default)]
    pub desired_hash: String,
    #[serde(default)]
    pub rules: Vec<PortForwardRule>,
}

impl Default for AgentPortForwardingConfig {
    fn default() -> Self {
        Self {
            schema_version: PORT_FORWARDING_SCHEMA_VERSION,
            desired_hash: String::new(),
            rules: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PortForwardCapabilityStatus {
    Supported,
    NftMissing,
    InsufficientPrivilege,
    InetNatUnsupported,
    ProbeFailed,
    #[default]
    Unknown,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct PortForwardCapability {
    #[serde(default)]
    pub status: PortForwardCapabilityStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nft_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl PortForwardCapability {
    pub fn supported(&self) -> bool {
        self.status == PortForwardCapabilityStatus::Supported
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PortForwardRuntimeStatus {
    Absent,
    Applied,
    Drifted,
    Unsupported,
    Failed,
    #[default]
    Unknown,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct PortForwardRuleRuntimeStat {
    pub rule_id: Uuid,
    pub revision: i64,
    #[serde(default)]
    pub nat_matches: u64,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct PortForwardRuntimeSnapshot {
    #[serde(default)]
    pub status: PortForwardRuntimeStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owned_table_present: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub desired_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nft_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ipv4_forwarding_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ipv6_forwarding_enabled: Option<bool>,
    #[serde(default)]
    pub rules: Vec<PortForwardRuleRuntimeStat>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    #[serde(default)]
    pub observed_unix: u64,
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum PortForwardValidationError {
    #[error("port 0 is not valid")]
    PortZero,
    #[error("port range start must not exceed its end")]
    RangeReversed,
    #[error("port expression is empty")]
    ExpressionEmpty,
    #[error("invalid port expression item: {0}")]
    ExpressionInvalid(String),
    #[error("a rule must contain at least one mapping")]
    MappingsEmpty,
    #[error("a rule exceeds the maximum mapping count")]
    MappingsTooMany,
    #[error("incoming port ranges overlap")]
    IncomingOverlap,
    #[error("target range must be one port or have the same size as its incoming range")]
    TargetCardinalityMismatch,
    #[error("target expression must be one port or contain one item for every incoming item")]
    TargetExpressionCountMismatch,
    #[error("rule name is empty")]
    NameEmpty,
    #[error("rule name exceeds {MAX_PORT_FORWARD_NAME_BYTES} bytes")]
    NameTooLong,
    #[error("target IP is not a usable unicast address")]
    TargetIpInvalid,
    #[error("port-forwarding schema version is unsupported")]
    SchemaUnsupported,
    #[error("port-forwarding desired hash is required when rules are present")]
    DesiredHashMissing,
    #[error("port-forwarding desired hash must be 64 lowercase hexadecimal characters")]
    DesiredHashInvalid,
    #[error("port-forwarding desired hash does not match its rules")]
    DesiredHashMismatch,
    #[error("too many active port-forward rules")]
    RulesTooMany,
    #[error("active port-forward rules exceed the nftables program complexity limit")]
    ProgramTooLarge,
    #[error("rule IDs must be unique")]
    DuplicateRuleId,
    #[error("enabled rules claim overlapping ports for the same family and protocol")]
    CrossRuleOverlap,
}

pub fn parse_port_expression(
    expression: &str,
) -> Result<Vec<PortRange>, PortForwardValidationError> {
    if expression.trim().is_empty() {
        return Err(PortForwardValidationError::ExpressionEmpty);
    }
    let mut ranges = Vec::new();
    for raw_item in expression.split(',') {
        let item = raw_item.trim();
        if item.is_empty() {
            return Err(PortForwardValidationError::ExpressionInvalid(
                raw_item.to_string(),
            ));
        }
        let range = if let Some((start, end)) = item.split_once('-') {
            if end.contains('-') {
                return Err(PortForwardValidationError::ExpressionInvalid(
                    item.to_string(),
                ));
            }
            PortRange::new(parse_port(start, item)?, parse_port(end, item)?)?
        } else {
            let port = parse_port(item, item)?;
            PortRange::new(port, port)?
        };
        ranges.push(range);
    }
    if ranges.len() > MAX_PORT_FORWARD_MAPPINGS {
        return Err(PortForwardValidationError::MappingsTooMany);
    }
    validate_non_overlapping(&ranges)?;
    Ok(ranges)
}

pub fn pair_port_expressions(
    incoming_expression: &str,
    target_expression: &str,
) -> Result<Vec<PortForwardMapping>, PortForwardValidationError> {
    let incoming = parse_port_expression(incoming_expression)?;
    let target = parse_port_expression(target_expression)?;
    let mappings: Vec<PortForwardMapping> = if target.len() == 1 && target[0].is_single() {
        incoming
            .into_iter()
            .map(|incoming| PortForwardMapping {
                incoming,
                target: target[0],
            })
            .collect()
    } else {
        if incoming.len() != target.len() {
            return Err(PortForwardValidationError::TargetExpressionCountMismatch);
        }
        incoming
            .into_iter()
            .zip(target)
            .map(|(incoming, target)| PortForwardMapping { incoming, target })
            .collect()
    };
    validate_mappings(&mappings)?;
    Ok(mappings)
}

pub fn validate_port_forward_rule(
    rule: &PortForwardRule,
) -> Result<(), PortForwardValidationError> {
    let name = rule.name.trim();
    if name.is_empty() {
        return Err(PortForwardValidationError::NameEmpty);
    }
    if name.len() > MAX_PORT_FORWARD_NAME_BYTES {
        return Err(PortForwardValidationError::NameTooLong);
    }
    validate_target_ip(rule.target_ip)?;
    validate_mappings(&rule.mappings)
}

pub fn validate_port_forwarding_config(
    config: &AgentPortForwardingConfig,
) -> Result<(), PortForwardValidationError> {
    if config.schema_version != PORT_FORWARDING_SCHEMA_VERSION {
        return Err(PortForwardValidationError::SchemaUnsupported);
    }
    if config.rules.len() > MAX_PORT_FORWARD_RULES {
        return Err(PortForwardValidationError::RulesTooMany);
    }
    if !config.rules.is_empty() && config.desired_hash.is_empty() {
        return Err(PortForwardValidationError::DesiredHashMissing);
    }
    if !config.desired_hash.is_empty()
        && (config.desired_hash.len() != 64
            || !config
                .desired_hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)))
    {
        return Err(PortForwardValidationError::DesiredHashInvalid);
    }
    if !config.desired_hash.is_empty()
        && config.desired_hash != port_forwarding_desired_hash(&config.rules)
    {
        return Err(PortForwardValidationError::DesiredHashMismatch);
    }
    let mut ids = BTreeSet::new();
    for rule in &config.rules {
        if !ids.insert(rule.id) {
            return Err(PortForwardValidationError::DuplicateRuleId);
        }
        validate_port_forward_rule(rule)?;
    }
    if estimated_nft_program_bytes(&config.rules) > MAX_PORT_FORWARD_NFT_SCRIPT_BYTES {
        return Err(PortForwardValidationError::ProgramTooLarge);
    }
    validate_cross_rule_overlaps(&config.rules)
}

pub fn port_forwarding_desired_hash(rules: &[PortForwardRule]) -> String {
    let mut canonical = rules.to_vec();
    canonical.sort_by_key(|rule| rule.id);
    crate::auth::payload_hash(&serde_json::to_vec(&canonical).unwrap_or_default())
}

pub fn validate_target_ip(target: IpAddr) -> Result<(), PortForwardValidationError> {
    let invalid = match target {
        IpAddr::V4(ip) => {
            ip.is_unspecified()
                || ip.is_loopback()
                || ip.is_multicast()
                || ip.is_link_local()
                || ip.is_broadcast()
        }
        IpAddr::V6(ip) => {
            ip.is_unspecified()
                || ip.is_loopback()
                || ip.is_multicast()
                || ip.is_unicast_link_local()
        }
    };
    if invalid {
        Err(PortForwardValidationError::TargetIpInvalid)
    } else {
        Ok(())
    }
}

fn validate_mappings(mappings: &[PortForwardMapping]) -> Result<(), PortForwardValidationError> {
    if mappings.is_empty() {
        return Err(PortForwardValidationError::MappingsEmpty);
    }
    if mappings.len() > MAX_PORT_FORWARD_MAPPINGS {
        return Err(PortForwardValidationError::MappingsTooMany);
    }
    let mut incoming = Vec::with_capacity(mappings.len());
    for mapping in mappings {
        mapping.incoming.validate()?;
        mapping.target.validate()?;
        if !mapping.target.is_single()
            && mapping.target.cardinality() != mapping.incoming.cardinality()
        {
            return Err(PortForwardValidationError::TargetCardinalityMismatch);
        }
        incoming.push(mapping.incoming);
    }
    validate_non_overlapping(&incoming)
}

fn validate_non_overlapping(ranges: &[PortRange]) -> Result<(), PortForwardValidationError> {
    let mut sorted = ranges.to_vec();
    sorted.sort_by_key(|range| (range.start, range.end));
    if sorted.windows(2).any(|pair| pair[0].overlaps(pair[1])) {
        return Err(PortForwardValidationError::IncomingOverlap);
    }
    Ok(())
}

fn validate_cross_rule_overlaps(
    rules: &[PortForwardRule],
) -> Result<(), PortForwardValidationError> {
    for (index, left) in rules.iter().enumerate() {
        for right in &rules[index + 1..] {
            if left.target_ip.is_ipv4() != right.target_ip.is_ipv4() {
                continue;
            }
            if !left
                .protocol
                .transports()
                .iter()
                .any(|transport| right.protocol.transports().contains(transport))
            {
                continue;
            }
            if left.mappings.iter().any(|left_mapping| {
                right
                    .mappings
                    .iter()
                    .any(|right_mapping| left_mapping.incoming.overlaps(right_mapping.incoming))
            }) {
                return Err(PortForwardValidationError::CrossRuleOverlap);
            }
        }
    }
    Ok(())
}

fn estimated_nft_program_bytes(rules: &[PortForwardRule]) -> usize {
    const BASE_BYTES: usize = 2 * 1024;
    const STATEMENT_BYTES: usize = 384;
    const MAP_BYTES: usize = 128;
    const MAP_ELEMENT_BYTES: usize = 20;

    rules.iter().fold(BASE_BYTES, |total, rule| {
        let transports = rule.protocol.transports().len();
        let statements = rule
            .mappings
            .len()
            .saturating_mul(transports)
            .saturating_mul(2)
            .saturating_mul(STATEMENT_BYTES);
        let maps = rule
            .mappings
            .iter()
            .filter(|mapping| !mapping.target.is_single())
            .fold(0_usize, |bytes, mapping| {
                bytes.saturating_add(
                    MAP_BYTES.saturating_add(
                        usize::try_from(mapping.incoming.cardinality())
                            .unwrap_or(usize::MAX)
                            .saturating_mul(MAP_ELEMENT_BYTES),
                    ),
                )
            })
            .saturating_mul(transports);
        total.saturating_add(statements).saturating_add(maps)
    })
}

fn parse_port(value: &str, item: &str) -> Result<u16, PortForwardValidationError> {
    value
        .trim()
        .parse::<u16>()
        .map_err(|_| PortForwardValidationError::ExpressionInvalid(item.to_string()))
}

const fn default_port_forwarding_schema_version() -> u16 {
    PORT_FORWARDING_SCHEMA_VERSION
}

const fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_single_many_and_corresponding_ranges() {
        assert_eq!(
            pair_port_expressions("80,443,1000-1002", "8080").unwrap(),
            vec![
                PortForwardMapping {
                    incoming: PortRange { start: 80, end: 80 },
                    target: PortRange {
                        start: 8080,
                        end: 8080
                    }
                },
                PortForwardMapping {
                    incoming: PortRange {
                        start: 443,
                        end: 443
                    },
                    target: PortRange {
                        start: 8080,
                        end: 8080
                    }
                },
                PortForwardMapping {
                    incoming: PortRange {
                        start: 1000,
                        end: 1002
                    },
                    target: PortRange {
                        start: 8080,
                        end: 8080
                    }
                }
            ]
        );
        assert!(pair_port_expressions("1000-1002,2000-2001", "3000-3002,4000-4001").is_ok());
    }

    #[test]
    fn rejects_ambiguous_or_overlapping_expressions() {
        assert_eq!(
            pair_port_expressions("1000-1002", "2000-2001").unwrap_err(),
            PortForwardValidationError::TargetCardinalityMismatch
        );
        assert_eq!(
            parse_port_expression("80,79-81").unwrap_err(),
            PortForwardValidationError::IncomingOverlap
        );
    }

    #[test]
    fn rejects_cross_rule_protocol_and_family_collisions() {
        let base = PortForwardRule {
            id: Uuid::new_v4(),
            revision: 1,
            name: "web".to_string(),
            protocol: PortForwardProtocol::Both,
            target_ip: "192.0.2.8".parse().unwrap(),
            mappings: pair_port_expressions("80", "8080").unwrap(),
            masquerade: true,
        };
        let mut conflicting = base.clone();
        conflicting.id = Uuid::new_v4();
        conflicting.name = "conflict".to_string();
        conflicting.protocol = PortForwardProtocol::Tcp;
        assert_eq!(
            validate_cross_rule_overlaps(&[base.clone(), conflicting]).unwrap_err(),
            PortForwardValidationError::CrossRuleOverlap
        );
        let mut ipv6 = base.clone();
        ipv6.id = Uuid::new_v4();
        ipv6.target_ip = "2001:db8::8".parse().unwrap();
        assert!(validate_cross_rule_overlaps(&[base, ipv6]).is_ok());
    }

    #[test]
    fn serde_defaults_keep_old_runtime_configs_valid() {
        let value: AgentPortForwardingConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(value, AgentPortForwardingConfig::default());
    }

    #[test]
    fn rejects_desired_state_that_would_render_an_oversized_program() {
        let mut rules = Vec::new();
        for rule_index in 0..43_u16 {
            let first = rule_index * 256 + 1;
            let mappings = (0..256_u16)
                .map(|offset| PortForwardMapping {
                    incoming: PortRange {
                        start: first + offset,
                        end: first + offset,
                    },
                    target: PortRange {
                        start: 8080,
                        end: 8080,
                    },
                })
                .collect::<Vec<_>>();
            rules.push(PortForwardRule {
                id: Uuid::new_v4(),
                revision: 1,
                name: format!("rule-{rule_index}"),
                protocol: PortForwardProtocol::Tcp,
                target_ip: "192.0.2.8".parse().unwrap(),
                mappings,
                masquerade: true,
            });
        }
        let config = AgentPortForwardingConfig {
            desired_hash: port_forwarding_desired_hash(&rules),
            rules,
            ..AgentPortForwardingConfig::default()
        };
        assert_eq!(
            validate_port_forwarding_config(&config).unwrap_err(),
            PortForwardValidationError::ProgramTooLarge
        );
    }
}
