use std::net::{IpAddr, SocketAddr};

use axum::http::HeaderMap;
use ipnet::IpNet;

const DEFAULT_TRUSTED_PROXY_CIDRS: &[&str] = &["0.0.0.0/0", "::/0"];

#[derive(Clone, Debug)]
pub(crate) struct TrustedProxyConfig {
    cidrs: Vec<IpNet>,
}

impl Default for TrustedProxyConfig {
    fn default() -> Self {
        Self::from_entries(DEFAULT_TRUSTED_PROXY_CIDRS.iter().copied())
            .expect("default proxy CIDRs are valid")
    }
}

impl TrustedProxyConfig {
    pub(crate) fn trust_none() -> Self {
        Self { cidrs: Vec::new() }
    }

    pub(crate) fn from_optional_entries(entries: Option<&[String]>) -> Result<Self, String> {
        match entries {
            Some(entries) => Self::from_entries(entries.iter().map(String::as_str)),
            None => Ok(Self::default()),
        }
    }

    pub(crate) fn from_env_csv(value: &str) -> Result<Self, String> {
        let entries = value
            .split(',')
            .map(str::trim)
            .filter(|entry| !entry.is_empty());
        Self::from_entries(entries)
    }

    fn from_entries<'a>(entries: impl IntoIterator<Item = &'a str>) -> Result<Self, String> {
        let mut cidrs = Vec::new();
        for entry in entries {
            let entry = entry.trim();
            if entry.is_empty() {
                continue;
            }
            let cidr = entry
                .parse::<IpNet>()
                .map_err(|error| format!("trusted_proxy_cidr_invalid:{entry}:{error}"))?;
            if !cidrs.iter().any(|stored| stored == &cidr) {
                cidrs.push(cidr);
            }
        }
        Ok(Self { cidrs })
    }

    pub(crate) fn resolve_client_ip(&self, peer: SocketAddr, headers: &HeaderMap) -> IpAddr {
        let peer_ip = peer.ip();
        if !self.is_trusted(peer_ip) {
            return peer_ip;
        }
        let Some(chain) = forwarded_for_chain(headers) else {
            return peer_ip;
        };
        for ip in chain.iter().rev() {
            if !self.is_trusted(*ip) {
                return *ip;
            }
        }
        chain.first().copied().unwrap_or(peer_ip)
    }

    fn is_trusted(&self, ip: IpAddr) -> bool {
        self.cidrs.iter().any(|cidr| cidr.contains(&ip))
    }
}

fn forwarded_for_chain(headers: &HeaderMap) -> Option<Vec<IpAddr>> {
    let value = headers.get("x-forwarded-for")?.to_str().ok()?;
    let mut chain = Vec::new();
    for part in value.split(',') {
        let part = part.trim();
        if part.is_empty() {
            return None;
        }
        let ip = part.parse::<IpAddr>().ok()?;
        chain.push(ip);
    }
    (!chain.is_empty()).then_some(chain)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", value.parse().unwrap());
        headers
    }

    #[test]
    fn trusted_loopback_peer_uses_forwarded_origin() {
        let config = TrustedProxyConfig::default();
        let peer = "127.0.0.1:44000".parse().unwrap();

        assert_eq!(
            config.resolve_client_ip(peer, &headers("198.51.100.10")),
            "198.51.100.10".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn untrusted_peer_cannot_spoof_forwarded_origin() {
        let config = TrustedProxyConfig::from_env_csv("127.0.0.0/8,::1/128").unwrap();
        let peer = "198.51.100.20:44000".parse().unwrap();

        assert_eq!(
            config.resolve_client_ip(peer, &headers("203.0.113.99")),
            "198.51.100.20".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn trusted_peer_uses_rightmost_untrusted_forwarded_address() {
        let config = TrustedProxyConfig::from_env_csv("127.0.0.0/8,::1/128").unwrap();
        let peer = "127.0.0.1:44000".parse().unwrap();

        assert_eq!(
            config.resolve_client_ip(peer, &headers("203.0.113.9, 198.51.100.10, 127.0.0.1")),
            "198.51.100.10".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn malformed_forwarded_header_falls_back_to_peer() {
        let config = TrustedProxyConfig::default();
        let peer = "127.0.0.1:44000".parse().unwrap();

        assert_eq!(
            config.resolve_client_ip(peer, &headers("203.0.113.9, unknown")),
            "127.0.0.1".parse::<IpAddr>().unwrap()
        );
    }
}
