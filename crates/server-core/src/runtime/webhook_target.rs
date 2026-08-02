use std::{
    collections::HashSet,
    env,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    time::Duration,
};

use anyhow::{Context, Result};
use reqwest::{redirect::Policy, Client, Url};
use url::Host;

pub const DEVELOPMENT_LOOPBACK_WEBHOOKS_ENV: &str = "VPSMAN_DEV_ALLOW_LOOPBACK_WEBHOOKS";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WebhookTargetPolicy {
    PublicHttps,
    PublicHttpsWithDevelopmentLoopbackHttp,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AddressRequirement {
    Public,
    Loopback,
}

#[derive(Clone, Debug)]
struct ResolvedWebhookTarget {
    url: Url,
    resolution_domain: Option<String>,
    addresses: Vec<SocketAddr>,
}

#[derive(Clone, Debug)]
pub struct PreparedWebhookTarget {
    client: Client,
    url: Url,
}

impl PreparedWebhookTarget {
    pub fn client(&self) -> &Client {
        &self.client
    }

    pub fn url(&self) -> &Url {
        &self.url
    }
}

pub fn validate_webhook_target(target: &str) -> Result<Url> {
    validate_webhook_target_with_policy(target, webhook_target_policy_from_env())
}

pub async fn prepare_webhook_target(
    target: &str,
    request_timeout: Duration,
) -> Result<PreparedWebhookTarget> {
    let policy = webhook_target_policy_from_env();
    let resolved = resolve_webhook_target(target, policy, request_timeout).await?;
    prepare_resolved_webhook_target(resolved, request_timeout)
}

fn prepare_resolved_webhook_target(
    resolved: ResolvedWebhookTarget,
    request_timeout: Duration,
) -> Result<PreparedWebhookTarget> {
    let mut client_builder = Client::builder()
        .timeout(request_timeout)
        .connect_timeout(request_timeout)
        .redirect(Policy::none())
        // A proxy would bypass the address validation and DNS pinning below.
        .no_proxy();
    if let Some(domain) = resolved.resolution_domain.as_deref() {
        client_builder = client_builder.resolve_to_addrs(domain, &resolved.addresses);
    }
    let client = client_builder
        .build()
        .context("failed to build pinned webhook client")?;
    Ok(PreparedWebhookTarget {
        client,
        url: resolved.url,
    })
}

fn webhook_target_policy_from_env() -> WebhookTargetPolicy {
    let allow_development_loopback = env::var(DEVELOPMENT_LOOPBACK_WEBHOOKS_ENV)
        .ok()
        .is_some_and(|value| matches!(value.trim().to_ascii_lowercase().as_str(), "1" | "true"));
    if allow_development_loopback {
        WebhookTargetPolicy::PublicHttpsWithDevelopmentLoopbackHttp
    } else {
        WebhookTargetPolicy::PublicHttps
    }
}

fn validate_webhook_target_with_policy(target: &str, policy: WebhookTargetPolicy) -> Result<Url> {
    let target = target.trim();
    anyhow::ensure!(!target.is_empty(), "webhook target is empty");
    let contains_userinfo = authority_contains_userinfo(target);
    let url = Url::parse(target).context("webhook target must be an absolute URL")?;
    anyhow::ensure!(
        url.username().is_empty() && url.password().is_none() && !contains_userinfo,
        "webhook target must not embed credentials"
    );
    anyhow::ensure!(
        url.fragment().is_none(),
        "webhook target must not contain a fragment"
    );
    anyhow::ensure!(
        url.port_or_known_default().is_some_and(|port| port != 0),
        "webhook target port is invalid"
    );

    let host = url.host().context("webhook target must include a host")?;
    match url.scheme() {
        "https" => validate_public_host(&host)?,
        "http"
            if policy == WebhookTargetPolicy::PublicHttpsWithDevelopmentLoopbackHttp
                && host_is_loopback(&host) => {}
        "http" => {
            anyhow::bail!(
                "webhook target must use https; local http requires explicit development opt-in"
            )
        }
        _ => anyhow::bail!("webhook target must use https"),
    }
    Ok(url)
}

async fn resolve_webhook_target(
    target: &str,
    policy: WebhookTargetPolicy,
    resolve_timeout: Duration,
) -> Result<ResolvedWebhookTarget> {
    anyhow::ensure!(
        !resolve_timeout.is_zero(),
        "webhook target timeout must be greater than zero"
    );
    let url = validate_webhook_target_with_policy(target, policy)?;
    let requirement = address_requirement(&url)?;
    let port = url
        .port_or_known_default()
        .context("webhook target port is invalid")?;
    match url.host().context("webhook target must include a host")? {
        Host::Ipv4(address) => {
            let addresses = validate_resolved_addresses(
                &url,
                [SocketAddr::new(IpAddr::V4(address), port)],
                requirement,
            )?;
            Ok(ResolvedWebhookTarget {
                url,
                resolution_domain: None,
                addresses,
            })
        }
        Host::Ipv6(address) => {
            let addresses = validate_resolved_addresses(
                &url,
                [SocketAddr::new(IpAddr::V6(address), port)],
                requirement,
            )?;
            Ok(ResolvedWebhookTarget {
                url,
                resolution_domain: None,
                addresses,
            })
        }
        Host::Domain(domain) => {
            let domain = domain.to_string();
            let lookup = tokio::time::timeout(
                resolve_timeout,
                tokio::net::lookup_host((domain.as_str(), port)),
            )
            .await
            .context("webhook target DNS resolution timed out")?
            .with_context(|| format!("failed to resolve webhook target host {domain}"))?;
            let addresses = validate_resolved_addresses(&url, lookup, requirement)?;
            Ok(ResolvedWebhookTarget {
                url,
                resolution_domain: Some(domain),
                addresses,
            })
        }
    }
}

fn validate_resolved_addresses(
    url: &Url,
    addresses: impl IntoIterator<Item = SocketAddr>,
    requirement: AddressRequirement,
) -> Result<Vec<SocketAddr>> {
    let port = url
        .port_or_known_default()
        .context("webhook target port is invalid")?;
    let mut unique = HashSet::new();
    let mut validated = Vec::new();
    for address in addresses {
        let address = SocketAddr::new(address.ip(), port);
        let allowed = match requirement {
            AddressRequirement::Public => is_public_ip(address.ip()),
            AddressRequirement::Loopback => address.ip().is_loopback(),
        };
        if !allowed {
            match requirement {
                AddressRequirement::Public => {
                    anyhow::bail!("webhook target resolved to a non-public address")
                }
                AddressRequirement::Loopback => {
                    anyhow::bail!("development webhook target resolved to a non-loopback address")
                }
            }
        }
        if unique.insert(address) {
            validated.push(address);
        }
    }
    anyhow::ensure!(
        !validated.is_empty(),
        "webhook target DNS resolution returned no addresses"
    );
    Ok(validated)
}

fn address_requirement(url: &Url) -> Result<AddressRequirement> {
    match url.scheme() {
        "https" => Ok(AddressRequirement::Public),
        "http" => Ok(AddressRequirement::Loopback),
        _ => anyhow::bail!("webhook target scheme is invalid"),
    }
}

fn authority_contains_userinfo(target: &str) -> bool {
    target
        .split_once("://")
        .and_then(|(_, remainder)| remainder.split(['/', '?', '#']).next())
        .is_some_and(|authority| authority.contains('@'))
}

fn validate_public_host(host: &Host<&str>) -> Result<()> {
    match host {
        Host::Ipv4(address) => anyhow::ensure!(
            is_public_ipv4(*address),
            "webhook target address is not public"
        ),
        Host::Ipv6(address) => anyhow::ensure!(
            is_public_ipv6(*address),
            "webhook target address is not public"
        ),
        Host::Domain(domain) => anyhow::ensure!(
            !is_reserved_domain(domain),
            "webhook target host is reserved or non-public"
        ),
    }
    Ok(())
}

fn host_is_loopback(host: &Host<&str>) -> bool {
    match host {
        Host::Ipv4(address) => address.is_loopback(),
        Host::Ipv6(address) => address.is_loopback(),
        Host::Domain(domain) => domain
            .trim_end_matches('.')
            .eq_ignore_ascii_case("localhost"),
    }
}

fn is_reserved_domain(domain: &str) -> bool {
    let domain = domain.trim_end_matches('.').to_ascii_lowercase();
    if domain.is_empty() || !domain.contains('.') {
        return true;
    }
    const RESERVED_SUFFIXES: &[&str] = &[
        "alt",
        "arpa",
        "example",
        "home.arpa",
        "internal",
        "invalid",
        "local",
        "localhost",
        "onion",
        "test",
    ];
    if RESERVED_SUFFIXES
        .iter()
        .any(|suffix| domain == *suffix || domain.ends_with(&format!(".{suffix}")))
    {
        return true;
    }
    ["example.com", "example.net", "example.org"]
        .iter()
        .any(|name| domain == *name || domain.ends_with(&format!(".{name}")))
}

fn is_public_ip(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => is_public_ipv4(address),
        IpAddr::V6(address) => is_public_ipv6(address),
    }
}

fn is_public_ipv4(address: Ipv4Addr) -> bool {
    let [a, b, c, _] = address.octets();
    !matches!(
        (a, b, c),
        (0, _, _)
            | (10, _, _)
            | (100, 64..=127, _)
            | (127, _, _)
            | (169, 254, _)
            | (172, 16..=31, _)
            | (192, 0, 0)
            | (192, 0, 2)
            | (192, 88, 99)
            | (192, 168, _)
            | (198, 18..=19, _)
            | (198, 51, 100)
            | (203, 0, 113)
            | (224..=255, _, _)
    )
}

fn is_public_ipv6(address: Ipv6Addr) -> bool {
    let value = u128::from(address);
    let allocated_global_unicast = [
        (Ipv6Addr::new(0x2001, 0x0200, 0, 0, 0, 0, 0, 0), 23),
        (Ipv6Addr::new(0x2001, 0x0400, 0, 0, 0, 0, 0, 0), 23),
        (Ipv6Addr::new(0x2001, 0x0600, 0, 0, 0, 0, 0, 0), 23),
        (Ipv6Addr::new(0x2001, 0x0800, 0, 0, 0, 0, 0, 0), 22),
        (Ipv6Addr::new(0x2001, 0x0c00, 0, 0, 0, 0, 0, 0), 23),
        (Ipv6Addr::new(0x2001, 0x0e00, 0, 0, 0, 0, 0, 0), 23),
        (Ipv6Addr::new(0x2001, 0x1200, 0, 0, 0, 0, 0, 0), 23),
        (Ipv6Addr::new(0x2001, 0x1400, 0, 0, 0, 0, 0, 0), 22),
        (Ipv6Addr::new(0x2001, 0x1800, 0, 0, 0, 0, 0, 0), 23),
        (Ipv6Addr::new(0x2001, 0x1a00, 0, 0, 0, 0, 0, 0), 23),
        (Ipv6Addr::new(0x2001, 0x1c00, 0, 0, 0, 0, 0, 0), 22),
        (Ipv6Addr::new(0x2001, 0x2000, 0, 0, 0, 0, 0, 0), 19),
        (Ipv6Addr::new(0x2001, 0x4000, 0, 0, 0, 0, 0, 0), 23),
        (Ipv6Addr::new(0x2001, 0x4200, 0, 0, 0, 0, 0, 0), 23),
        (Ipv6Addr::new(0x2001, 0x4400, 0, 0, 0, 0, 0, 0), 23),
        (Ipv6Addr::new(0x2001, 0x4600, 0, 0, 0, 0, 0, 0), 23),
        (Ipv6Addr::new(0x2001, 0x4800, 0, 0, 0, 0, 0, 0), 23),
        (Ipv6Addr::new(0x2001, 0x4a00, 0, 0, 0, 0, 0, 0), 23),
        (Ipv6Addr::new(0x2001, 0x4c00, 0, 0, 0, 0, 0, 0), 23),
        (Ipv6Addr::new(0x2001, 0x5000, 0, 0, 0, 0, 0, 0), 20),
        (Ipv6Addr::new(0x2001, 0x8000, 0, 0, 0, 0, 0, 0), 19),
        (Ipv6Addr::new(0x2001, 0xa000, 0, 0, 0, 0, 0, 0), 20),
        (Ipv6Addr::new(0x2001, 0xb000, 0, 0, 0, 0, 0, 0), 20),
        (Ipv6Addr::new(0x2003, 0, 0, 0, 0, 0, 0, 0), 18),
        (Ipv6Addr::new(0x2400, 0, 0, 0, 0, 0, 0, 0), 12),
        (Ipv6Addr::new(0x2410, 0, 0, 0, 0, 0, 0, 0), 12),
        (Ipv6Addr::new(0x2600, 0, 0, 0, 0, 0, 0, 0), 12),
        (Ipv6Addr::new(0x2610, 0, 0, 0, 0, 0, 0, 0), 23),
        (Ipv6Addr::new(0x2620, 0, 0, 0, 0, 0, 0, 0), 23),
        (Ipv6Addr::new(0x2630, 0, 0, 0, 0, 0, 0, 0), 12),
        (Ipv6Addr::new(0x2800, 0, 0, 0, 0, 0, 0, 0), 12),
        (Ipv6Addr::new(0x2a00, 0, 0, 0, 0, 0, 0, 0), 12),
        (Ipv6Addr::new(0x2a10, 0, 0, 0, 0, 0, 0, 0), 12),
        (Ipv6Addr::new(0x2c00, 0, 0, 0, 0, 0, 0, 0), 12),
    ];
    allocated_global_unicast
        .iter()
        .any(|(prefix, bits)| in_ipv6_prefix(value, u128::from(*prefix), *bits))
        && !in_ipv6_prefix(
            value,
            u128::from(Ipv6Addr::new(0x2001, 0x0db8, 0, 0, 0, 0, 0, 0)),
            32,
        )
        && !in_ipv6_prefix(
            value,
            u128::from(Ipv6Addr::new(0x2620, 0x004f, 0x8000, 0, 0, 0, 0, 0)),
            48,
        )
}

fn in_ipv6_prefix(address: u128, prefix: u128, prefix_bits: u32) -> bool {
    let shift = 128_u32.saturating_sub(prefix_bits);
    address >> shift == prefix >> shift
}

#[cfg(test)]
#[path = "tests_webhook_target.rs"]
mod tests;
