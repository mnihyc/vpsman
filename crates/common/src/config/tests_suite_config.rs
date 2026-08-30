use serde_json::json;

use super::{redact_suite_config_value, SuiteConfig};

#[test]
fn suite_config_bounds_gateway_telemetry_http_ownership() {
    let configured = SuiteConfig::parse(
        r#"
version = 1

[capacity]
gateway_telemetry_in_flight = 8
"#,
    )
    .expect("bounded gateway telemetry HTTP ownership");
    assert_eq!(configured.capacity.gateway_telemetry_in_flight, Some(8));
    assert!(configured
        .validation_summary()
        .restart_required_fields
        .contains(&"capacity.gateway_telemetry_in_flight".to_string()));

    for invalid in [0, 513] {
        let error = SuiteConfig::parse(&format!(
            "version = 1\n\n[capacity]\ngateway_telemetry_in_flight = {invalid}\n"
        ))
        .unwrap_err();
        assert_eq!(error, "capacity.gateway_telemetry_in_flight_out_of_range");
    }
}

#[test]
fn suite_config_redaction_hides_credential_urls_and_sensitive_keys() {
    let redacted = redact_suite_config_value(json!({
        "database": {
            "postgres_url": "postgres://vpsman:secret@postgres:5432/vpsman",
            "migrations_dir": "migrations"
        },
        "webhooks": {
            "callback": "https://token@example.test/hook"
        },
        "gateway": {
            "expect_client_public_key_hex": "abcd"
        }
    }));

    assert_eq!(redacted["database"]["postgres_url"], "<redacted>");
    assert_eq!(redacted["webhooks"]["callback"], "<redacted>");
    assert_eq!(
        redacted["gateway"]["expect_client_public_key_hex"],
        "<redacted>"
    );
    assert_eq!(redacted["database"]["migrations_dir"], "migrations");
}

#[test]
fn suite_config_redaction_keeps_secret_file_refs_and_plain_internal_urls() {
    let redacted = redact_suite_config_value(json!({
        "api": {
            "gateway_control_url": "http://gateway:9444"
        },
        "gateway": {
            "api_url": "http://api:8080"
        },
        "secrets": {
            "internal_token_file": "/run/secrets/vpsman_internal_token",
            "object_secret_key_file": "/run/secrets/object_secret_key"
        }
    }));

    assert_eq!(
        redacted["api"]["gateway_control_url"],
        "http://gateway:9444"
    );
    assert_eq!(redacted["gateway"]["api_url"], "http://api:8080");
    assert_eq!(
        redacted["secrets"]["internal_token_file"],
        "/run/secrets/vpsman_internal_token"
    );
    assert_eq!(
        redacted["secrets"]["object_secret_key_file"],
        "/run/secrets/object_secret_key"
    );
}

#[test]
fn suite_config_accepts_ipv4_and_ipv6_trusted_proxy_cidrs() {
    let config = SuiteConfig::parse(
        r#"
version = 1

[api]
trusted_proxy_cidrs = ["127.0.0.0/8", "::1/128", "2001:db8::/32"]
"#,
    )
    .expect("valid CIDRs");

    assert_eq!(
        config.api.trusted_proxy_cidrs,
        Some(vec![
            "127.0.0.0/8".to_string(),
            "::1/128".to_string(),
            "2001:db8::/32".to_string(),
        ])
    );
}

#[test]
fn suite_config_rejects_invalid_trusted_proxy_cidr() {
    let error = SuiteConfig::parse(
        r#"
version = 1

[api]
trusted_proxy_cidrs = ["localhost"]
"#,
    )
    .unwrap_err();

    assert_eq!(error, "api.trusted_proxy_cidrs_invalid");
}

#[test]
fn suite_config_accepts_empty_or_valid_tunnel_allocation_pools() {
    let empty = SuiteConfig::parse(
        r#"
version = 1

[network]
tunnel_ipv4_allocation_pool_cidr = ""
tunnel_ipv6_allocation_pool_cidr = ""
"#,
    )
    .expect("empty pools are disabled");
    assert_eq!(
        empty.network.tunnel_ipv4_allocation_pool_cidr.as_deref(),
        Some("")
    );

    let configured = SuiteConfig::parse(
        r#"
version = 1

[network]
tunnel_ipv4_allocation_pool_cidr = "10.255.0.0/16"
tunnel_ipv6_allocation_pool_cidr = "fd80::/80"
"#,
    )
    .expect("valid pools");
    assert_eq!(
        configured
            .network
            .tunnel_ipv4_allocation_pool_cidr
            .as_deref(),
        Some("10.255.0.0/16")
    );
    assert_eq!(
        configured
            .network
            .tunnel_ipv6_allocation_pool_cidr
            .as_deref(),
        Some("fd80::/80")
    );
}

#[test]
fn suite_config_rejects_invalid_tunnel_allocation_pools() {
    let wrong_family = SuiteConfig::parse(
        r#"
version = 1

[network]
tunnel_ipv4_allocation_pool_cidr = "fd80::/80"
"#,
    )
    .unwrap_err();
    assert_eq!(
        wrong_family,
        "network.tunnel_ipv4_allocation_pool_cidr_wrong_family"
    );

    let too_small = SuiteConfig::parse(
        r#"
version = 1

[network]
tunnel_ipv6_allocation_pool_cidr = "fd80::/128"
"#,
    )
    .unwrap_err();
    assert_eq!(
        too_small,
        "network.tunnel_ipv6_allocation_pool_cidr_too_small"
    );
}

#[test]
fn suite_config_rejects_retired_resource_alert_threshold_keys() {
    let error = SuiteConfig::parse(
        r#"
version = 1

[api]
alert_memory_available_warning_ratio = 0.1
alert_memory_available_critical_ratio = 0.2
alert_disk_available_warning_ratio = 1.0
alert_disk_available_critical_ratio = 0.0
alert_cpu_load_warning = 4.0
alert_cpu_load_critical = 2.0
"#,
    )
    .expect_err("retired resource alert threshold keys must not remain parse-compatible");

    assert!(error.contains("unknown field `alert_memory_available_warning_ratio`"));
}
