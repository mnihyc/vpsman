use std::{collections::BTreeSet, net::IpAddr};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

pub const PORT_FORWARDING_SCHEMA_VERSION: u16 = 1;
pub const PORT_FORWARDING_MODES_SCHEMA_VERSION: u16 = 2;
pub const MAX_PORT_FORWARD_RULES: usize = 512;
pub const MAX_PORT_FORWARD_MAPPINGS: usize = 256;
pub const MAX_PORT_FORWARD_NAME_BYTES: usize = 128;
pub const MAX_PORT_FORWARD_NFT_SCRIPT_BYTES: usize = 8 * 1024 * 1024;

#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize,
)]
#[serde(rename_all = "snake_case")]
pub enum PortForwardMode {
    #[default]
    Dnat,
    Redirect,
    CustomAdapter,
}

impl PortForwardMode {
    fn is_dnat(&self) -> bool {
        *self == Self::Dnat
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PortForwardAddressFamily {
    Ipv4,
    Ipv6,
    Both,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PortForwardAdapterCommands {
    #[serde(rename = "template_id")]
    pub definition_id: Uuid,
    #[serde(rename = "template_name")]
    pub definition_name: String,
    pub definition_hash: String,
    pub apply: crate::RuntimeTunnelCommand,
    pub remove: crate::RuntimeTunnelCommand,
    pub status: crate::RuntimeTunnelCommand,
}

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_ip: Option<IpAddr>,
    pub mappings: Vec<PortForwardMapping>,
    #[serde(default = "default_true")]
    pub masquerade: bool,
    #[serde(default, skip_serializing_if = "PortForwardMode::is_dnat")]
    pub mode: PortForwardMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address_family: Option<PortForwardAddressFamily>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adapter: Option<PortForwardAdapterCommands>,
}

impl PortForwardRule {
    /// Built-in dispatch claims only; custom adapters own their own listeners.
    pub fn native_families(&self) -> &'static [PortForwardAddressFamily] {
        use PortForwardAddressFamily::{Both, Ipv4, Ipv6};
        match self.mode {
            PortForwardMode::Dnat => match self.target_ip {
                Some(IpAddr::V4(_)) => &[Ipv4],
                Some(IpAddr::V6(_)) => &[Ipv6],
                None => &[],
            },
            PortForwardMode::Redirect => match self.address_family {
                Some(Ipv4) => &[Ipv4],
                Some(Ipv6) => &[Ipv6],
                Some(Both) => &[Ipv4, Ipv6],
                None => &[],
            },
            PortForwardMode::CustomAdapter => &[],
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PortForwardCleanupRule {
    pub rule_id: Uuid,
    pub revision: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentPortForwardingConfig {
    #[serde(default = "default_port_forwarding_schema_version")]
    pub schema_version: u16,
    #[serde(default)]
    pub desired_hash: String,
    #[serde(default)]
    pub rules: Vec<PortForwardRule>,
    /// Custom owners whose absence must be acknowledged independently of nftables.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cleanup_rules: Vec<PortForwardCleanupRule>,
}

impl Default for AgentPortForwardingConfig {
    fn default() -> Self {
        Self {
            schema_version: PORT_FORWARDING_SCHEMA_VERSION,
            desired_hash: String::new(),
            rules: Vec::new(),
            cleanup_rules: Vec::new(),
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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PortForwardCapability {
    #[serde(default)]
    pub status: PortForwardCapabilityStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nft_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default = "default_port_forwarding_schema_version")]
    pub schema_version: u16,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub supported_modes: Vec<PortForwardMode>,
}

impl Default for PortForwardCapability {
    fn default() -> Self {
        Self {
            status: PortForwardCapabilityStatus::Unknown,
            nft_version: None,
            reason: None,
            schema_version: PORT_FORWARDING_SCHEMA_VERSION,
            supported_modes: Vec::new(),
        }
    }
}

impl PortForwardCapability {
    pub fn supported(&self) -> bool {
        self.status == PortForwardCapabilityStatus::Supported
    }

    pub fn supports_mode(&self, mode: PortForwardMode) -> bool {
        match mode {
            PortForwardMode::Dnat => self.supported(),
            PortForwardMode::Redirect => {
                self.supported()
                    && self.schema_version >= PORT_FORWARDING_MODES_SCHEMA_VERSION
                    && self.supported_modes.contains(&mode)
            }
            PortForwardMode::CustomAdapter => {
                self.schema_version >= PORT_FORWARDING_MODES_SCHEMA_VERSION
                    && self.supported_modes.contains(&mode)
            }
        }
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
    pub nat_matches: Option<u64>,
    #[serde(default, skip_serializing_if = "PortForwardMode::is_dnat")]
    pub mode: PortForwardMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<PortForwardRuntimeStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_unix: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
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
    pub native_desired_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub removed_rules: Vec<PortForwardCleanupRule>,
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
    #[error("fields do not match the selected port-forward mode")]
    ModeFieldsInvalid,
    #[error("custom port-forward adapter command is invalid")]
    AdapterCommandInvalid,
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
    #[error(
        "cleanup rules require positive revisions and unique IDs not naming enabled custom rules"
    )]
    CleanupRuleIdInvalid,
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
    match rule.mode {
        PortForwardMode::Dnat => {
            validate_target_ip(
                rule.target_ip
                    .ok_or(PortForwardValidationError::TargetIpInvalid)?,
            )?;
            if rule.adapter.is_some() || rule.address_family.is_some() {
                return Err(PortForwardValidationError::ModeFieldsInvalid);
            }
        }
        PortForwardMode::Redirect => {
            if rule.target_ip.is_some()
                || rule.adapter.is_some()
                || rule.address_family.is_none()
                || rule.masquerade
            {
                return Err(PortForwardValidationError::ModeFieldsInvalid);
            }
        }
        PortForwardMode::CustomAdapter => {
            if rule.address_family.is_some() || rule.masquerade {
                return Err(PortForwardValidationError::ModeFieldsInvalid);
            }
            let adapter = rule
                .adapter
                .as_ref()
                .ok_or(PortForwardValidationError::ModeFieldsInvalid)?;
            // Share the existing adapter command budget and direct-argv contract.
            for command in [&adapter.apply, &adapter.remove, &adapter.status] {
                if command.argv.is_empty()
                    || command.argv.len() > 32
                    || !std::path::Path::new(&command.argv[0]).is_absolute()
                    || command
                        .argv
                        .iter()
                        .any(|arg| arg.is_empty() || arg.len() > 4096 || arg.contains('\0'))
                    || !(1..=120).contains(&command.max_timeout_secs)
                    || !(1024..=65536).contains(&command.max_output_bytes)
                {
                    return Err(PortForwardValidationError::AdapterCommandInvalid);
                }
            }
        }
    }
    validate_mappings(&rule.mappings)
}

pub fn validate_port_forwarding_config(
    config: &AgentPortForwardingConfig,
) -> Result<(), PortForwardValidationError> {
    if !matches!(
        config.schema_version,
        PORT_FORWARDING_SCHEMA_VERSION | PORT_FORWARDING_MODES_SCHEMA_VERSION
    ) || (config.schema_version == PORT_FORWARDING_SCHEMA_VERSION
        && (!config.cleanup_rules.is_empty()
            || config
                .rules
                .iter()
                .any(|rule| rule.mode != PortForwardMode::Dnat)))
    {
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
    let mut cleanup_ids = BTreeSet::new();
    for cleanup in &config.cleanup_rules {
        if cleanup.revision <= 0
            || !cleanup_ids.insert(cleanup.rule_id)
            || config.rules.iter().any(|rule| {
                rule.id == cleanup.rule_id && rule.mode == PortForwardMode::CustomAdapter
            })
        {
            return Err(PortForwardValidationError::CleanupRuleIdInvalid);
        }
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
    let mut claims = rules
        .iter()
        .flat_map(|rule| {
            rule.native_families().iter().flat_map(move |family| {
                rule.protocol
                    .transports()
                    .iter()
                    .flat_map(move |transport| {
                        rule.mappings.iter().map(move |mapping| {
                            (
                                *family == PortForwardAddressFamily::Ipv6,
                                *transport,
                                mapping.incoming,
                            )
                        })
                    })
            })
        })
        .collect::<Vec<_>>();
    claims.sort_unstable_by_key(|(ipv6, transport, range)| {
        (*ipv6, *transport, range.start, range.end)
    });
    if claims.windows(2).any(|pair| {
        pair[0].0 == pair[1].0 && pair[0].1 == pair[1].1 && pair[0].2.overlaps(pair[1].2)
    }) {
        return Err(PortForwardValidationError::CrossRuleOverlap);
    }
    Ok(())
}

fn estimated_nft_program_bytes(rules: &[PortForwardRule]) -> usize {
    const BASE_BYTES: usize = 2 * 1024;
    const RULE_PROGRAM_BYTES: usize = 768;
    const DISPATCH_ELEMENT_BYTES: usize = 96;
    const COMPACT_MAP_ELEMENT_BYTES: usize = 48;
    const MAP_ELEMENT_BYTES: usize = 20;

    rules
        .iter()
        .filter(|rule| rule.mode != PortForwardMode::CustomAdapter)
        .fold(BASE_BYTES, |total, rule| {
            let transports = rule.protocol.transports().len() * rule.native_families().len();
            let compact_elements = rule
                .mappings
                .iter()
                .filter(|mapping| mapping.target.is_single() && mapping.incoming != mapping.target)
                .count()
                .saturating_mul(COMPACT_MAP_ELEMENT_BYTES);
            let shifted_elements = rule
                .mappings
                .iter()
                .filter(|mapping| !mapping.target.is_single() && mapping.incoming != mapping.target)
                .fold(0_usize, |bytes, mapping| {
                    bytes.saturating_add(
                        usize::try_from(mapping.incoming.cardinality())
                            .unwrap_or(usize::MAX)
                            .saturating_mul(MAP_ELEMENT_BYTES),
                    )
                });
            let per_transport = RULE_PROGRAM_BYTES
                .saturating_add(rule.mappings.len().saturating_mul(DISPATCH_ELEMENT_BYTES))
                .saturating_add(compact_elements)
                .saturating_add(shifted_elements);
            total.saturating_add(per_transport.saturating_mul(transports))
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
#[path = "tests_port_forwarding.rs"]
mod tests;
