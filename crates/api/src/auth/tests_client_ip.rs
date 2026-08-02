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
    let config = TrustedProxyConfig::default();
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
