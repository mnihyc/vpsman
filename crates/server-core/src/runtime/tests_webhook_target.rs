use super::*;

fn secure() -> WebhookTargetPolicy {
    WebhookTargetPolicy::PublicHttps
}

fn development() -> WebhookTargetPolicy {
    WebhookTargetPolicy::PublicHttpsWithDevelopmentLoopbackHttp
}

#[test]
fn secure_policy_requires_public_https_without_credentials() {
    assert!(validate_webhook_target_with_policy("https://hooks.acme.com/vpsman", secure()).is_ok());
    for target in [
        "http://hooks.acme.com/vpsman",
        "https://user:secret@hooks.acme.com/vpsman",
        "https://@hooks.acme.com/vpsman",
        "https://hooks.acme.com/vpsman#fragment",
        "https://localhost/vpsman",
        "https://10.0.0.1/vpsman",
        "https://2130706433/vpsman",
        "https://0x7f000001/vpsman",
        "https://[::1]/vpsman",
        "https://hooks.example.com/vpsman",
        "https://service.internal/vpsman",
        "https://single-label/vpsman",
    ] {
        assert!(
            validate_webhook_target_with_policy(target, secure()).is_err(),
            "{target} unexpectedly passed secure webhook validation"
        );
    }
}

#[test]
fn development_opt_in_is_limited_to_http_loopback() {
    for target in [
        "http://localhost:9000/hook",
        "http://127.0.0.1:9000/hook",
        "http://[::1]:9000/hook",
        "https://hooks.acme.com/hook",
    ] {
        assert!(
            validate_webhook_target_with_policy(target, development()).is_ok(),
            "{target} unexpectedly failed development webhook validation"
        );
    }
    for target in [
        "http://hooks.acme.com/hook",
        "http://10.0.0.1/hook",
        "https://127.0.0.1/hook",
        "http://service.local/hook",
    ] {
        assert!(
            validate_webhook_target_with_policy(target, development()).is_err(),
            "{target} unexpectedly passed development webhook validation"
        );
    }
}

#[test]
fn public_address_classification_rejects_special_ipv4_ranges() {
    for address in [
        "0.0.0.0",
        "10.1.2.3",
        "100.64.0.1",
        "127.0.0.1",
        "169.254.1.1",
        "172.16.0.1",
        "192.0.0.9",
        "192.0.2.1",
        "192.88.99.1",
        "192.168.1.1",
        "198.18.0.1",
        "198.51.100.1",
        "203.0.113.1",
        "224.0.0.1",
        "255.255.255.255",
    ] {
        let address: Ipv4Addr = address.parse().unwrap();
        assert!(
            !is_public_ipv4(address),
            "{address} unexpectedly classified as public"
        );
    }
    for address in ["1.1.1.1", "8.8.8.8", "93.184.216.34"] {
        let address: Ipv4Addr = address.parse().unwrap();
        assert!(
            is_public_ipv4(address),
            "{address} unexpectedly classified as non-public"
        );
    }
}

#[test]
fn public_address_classification_rejects_special_ipv6_ranges() {
    for address in [
        "::",
        "::1",
        "::ffff:127.0.0.1",
        "64:ff9b::1",
        "100::1",
        "2001::1",
        "2001:db8::1",
        "2002:7f00:1::1",
        "2d00::1",
        "3000::1",
        "3ffe::1",
        "3fff::1",
        "fc00::1",
        "fe80::1",
        "ff00::1",
    ] {
        let address: Ipv6Addr = address.parse().unwrap();
        assert!(
            !is_public_ipv6(address),
            "{address} unexpectedly classified as public"
        );
    }
    for address in ["2606:4700:4700::1111", "2a00:1450:4001:801::200e"] {
        let address: Ipv6Addr = address.parse().unwrap();
        assert!(
            is_public_ipv6(address),
            "{address} unexpectedly classified as non-public"
        );
    }
}

#[test]
fn resolution_rejects_mixed_or_non_public_answers_without_network() {
    let url =
        validate_webhook_target_with_policy("https://hooks.acme.com:8443/hook", secure()).unwrap();
    let public: SocketAddr = "1.1.1.1:443".parse().unwrap();
    let duplicate_with_different_port: SocketAddr = "1.1.1.1:9443".parse().unwrap();
    let private: SocketAddr = "10.0.0.1:443".parse().unwrap();

    let addresses = validate_resolved_addresses(
        &url,
        [public, duplicate_with_different_port],
        AddressRequirement::Public,
    )
    .unwrap();
    assert_eq!(addresses, vec!["1.1.1.1:8443".parse().unwrap()]);
    assert!(
        validate_resolved_addresses(&url, [public, private], AddressRequirement::Public).is_err()
    );
    assert!(validate_resolved_addresses(&url, [], AddressRequirement::Public).is_err());
}

#[test]
fn development_hostname_resolution_must_remain_loopback() {
    let url =
        validate_webhook_target_with_policy("http://localhost:9000/hook", development()).unwrap();
    assert!(validate_resolved_addresses(
        &url,
        [
            "127.0.0.1:9000".parse().unwrap(),
            "[::1]:9000".parse().unwrap()
        ],
        AddressRequirement::Loopback
    )
    .is_ok());
    assert!(validate_resolved_addresses(
        &url,
        ["1.1.1.1:9000".parse().unwrap()],
        AddressRequirement::Loopback
    )
    .is_err());
}

#[test]
fn pinned_client_keeps_the_original_https_authority_without_network() {
    let url =
        validate_webhook_target_with_policy("https://hooks.acme.com:8443/hook", secure()).unwrap();
    let resolved = ResolvedWebhookTarget {
        url,
        resolution_domain: Some("hooks.acme.com".to_string()),
        addresses: vec!["1.1.1.1:8443".parse().unwrap()],
    };

    let prepared = prepare_resolved_webhook_target(resolved, Duration::from_secs(5)).unwrap();
    assert_eq!(prepared.url().host_str(), Some("hooks.acme.com"));
    assert_eq!(prepared.url().port(), Some(8443));
}
