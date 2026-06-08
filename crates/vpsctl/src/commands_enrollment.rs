use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::PathBuf,
};

use anyhow::{Context, Result};
use serde::Deserialize;
use vpsman_common::{
    generate_noise_keypair, AgentAuthConfig, AgentBackupConfig, AgentConfig, AgentExecutionConfig,
    AgentNetworkConfig, AgentNoiseConfig, AgentNoiseMode, AgentTelemetryConfig, AgentUpdateConfig,
    ServerEndpoint,
};

use crate::http::{http_get, http_post_json, http_put_json};

pub(crate) fn enrollment_tokens(api_url: &str, token: Option<&str>) -> Result<()> {
    println!("{}", http_get(api_url, "/api/v1/enrollment-tokens", token)?);
    Ok(())
}

pub(crate) fn enrollment_settings(api_url: &str, token: Option<&str>) -> Result<()> {
    println!(
        "{}",
        http_get(api_url, "/api/v1/enrollment-settings", token)?
    );
    Ok(())
}

pub(crate) fn enrollment_settings_update(
    api_url: &str,
    token: Option<&str>,
    settings_file: PathBuf,
) -> Result<()> {
    let raw = fs::read_to_string(&settings_file)
        .with_context(|| format!("failed to read {}", settings_file.display()))?;
    let value: serde_json::Value = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse JSON from {}", settings_file.display()))?;
    let value = normalize_enrollment_settings_update_value(value);
    println!(
        "{}",
        http_put_json(api_url, "/api/v1/enrollment-settings", token, &value)?
    );
    Ok(())
}

fn normalize_enrollment_settings_update_value(mut value: serde_json::Value) -> serde_json::Value {
    if let Some(object) = value.as_object_mut() {
        object.remove("server_ed25519_public_key_hex");
    }
    value
}

pub(crate) struct EnrollmentTokenCreateOptions {
    pub(crate) ttl_secs: u64,
    pub(crate) default_tags: Vec<String>,
    pub(crate) default_display_name: Option<String>,
    pub(crate) unmanaged_update_enabled: bool,
    pub(crate) unmanaged_update_version_url: Option<String>,
    pub(crate) unmanaged_update_interval_secs: u64,
    pub(crate) unmanaged_update_jitter_secs: u64,
    pub(crate) unmanaged_update_activate: bool,
    pub(crate) unmanaged_update_restart_agent: bool,
}

pub(crate) fn enrollment_token_create(
    api_url: &str,
    token: Option<&str>,
    options: EnrollmentTokenCreateOptions,
) -> Result<()> {
    println!(
        "{}",
        http_post_json(
            api_url,
            "/api/v1/enrollment-tokens",
            token,
            &serde_json::json!({
                "ttl_secs": options.ttl_secs,
                "purpose": "provision",
                "allowed_client_id": null,
                "confirmed_reenrollment": false,
                "preserve_existing_assignments": true,
                "default_tags": options.default_tags,
                "default_display_name": options.default_display_name,
                "unmanaged_update_enabled": options.unmanaged_update_enabled,
                "unmanaged_update_version_url": options.unmanaged_update_version_url,
                "unmanaged_update_interval_secs": options.unmanaged_update_interval_secs,
                "unmanaged_update_jitter_secs": options.unmanaged_update_jitter_secs,
                "unmanaged_update_activate": options.unmanaged_update_activate,
                "unmanaged_update_restart_agent": options.unmanaged_update_restart_agent,
            }),
        )?
    );
    Ok(())
}

pub(crate) struct ReenrollmentTokenCreateOptions {
    pub(crate) client_id: String,
    pub(crate) ttl_secs: u64,
    pub(crate) default_tags: Vec<String>,
    pub(crate) default_display_name: Option<String>,
    pub(crate) confirmed: bool,
    pub(crate) preserve_existing_assignments: bool,
    pub(crate) unmanaged_update_enabled: bool,
    pub(crate) unmanaged_update_version_url: Option<String>,
    pub(crate) unmanaged_update_interval_secs: u64,
    pub(crate) unmanaged_update_jitter_secs: u64,
    pub(crate) unmanaged_update_activate: bool,
    pub(crate) unmanaged_update_restart_agent: bool,
}

pub(crate) fn reenrollment_token_create(
    api_url: &str,
    token: Option<&str>,
    options: ReenrollmentTokenCreateOptions,
) -> Result<()> {
    println!(
        "{}",
        http_post_json(
            api_url,
            "/api/v1/enrollment-tokens",
            token,
            &serde_json::json!({
                "ttl_secs": options.ttl_secs,
                "purpose": "rebuild_reenrollment",
                "allowed_client_id": options.client_id,
                "confirmed_reenrollment": options.confirmed,
                "preserve_existing_assignments": options.preserve_existing_assignments,
                "default_tags": options.default_tags,
                "default_display_name": options.default_display_name,
                "unmanaged_update_enabled": options.unmanaged_update_enabled,
                "unmanaged_update_version_url": options.unmanaged_update_version_url,
                "unmanaged_update_interval_secs": options.unmanaged_update_interval_secs,
                "unmanaged_update_jitter_secs": options.unmanaged_update_jitter_secs,
                "unmanaged_update_activate": options.unmanaged_update_activate,
                "unmanaged_update_restart_agent": options.unmanaged_update_restart_agent,
            }),
        )?
    );
    Ok(())
}

pub(crate) fn client_key_revocations(api_url: &str, token: Option<&str>, limit: u16) -> Result<()> {
    println!(
        "{}",
        http_get(
            api_url,
            &format!("/api/v1/client-key-revocations?limit={limit}"),
            token
        )?
    );
    Ok(())
}

pub(crate) fn client_key_revoke(
    api_url: &str,
    token: Option<&str>,
    client_id: String,
    reason: Option<String>,
    confirmed: bool,
) -> Result<()> {
    println!(
        "{}",
        http_post_json(
            api_url,
            &format!(
                "/api/v1/clients/{}/key-revocations",
                encode_path(&client_id)
            ),
            token,
            &serde_json::json!({
                "confirmed": confirmed,
                "reason": reason,
            }),
        )?
    );
    Ok(())
}

pub(crate) fn key_lifecycle_report(api_url: &str, token: Option<&str>) -> Result<()> {
    println!(
        "{}",
        http_get(api_url, "/api/v1/key-lifecycle/report", token)?
    );
    Ok(())
}

fn encode_path(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(byte as char);
        } else {
            use std::fmt::Write as _;
            let _ = write!(&mut encoded, "%{byte:02X}");
        }
    }
    encoded
}

pub(crate) fn enroll_claim(
    api_url: &str,
    enrollment_token: String,
    client_public_key_hex: String,
) -> Result<()> {
    let body = serde_json::json!({
        "token": enrollment_token,
        "client_public_key_hex": client_public_key_hex,
    });
    println!(
        "{}",
        http_post_json(api_url, "/api/v1/enrollments/claim", None, &body,)?
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn enroll_config(
    api_url: &str,
    enrollment_token: String,
    command_timeout_secs: u64,
    output: Option<PathBuf>,
) -> Result<()> {
    let keypair = generate_noise_keypair()?;
    let response = claim_enrollment(api_url, enrollment_token, keypair.public_hex())?;
    let config = render_agent_config(&response, keypair.private_hex(), command_timeout_secs)?;
    let rendered = toml::to_string_pretty(&config).context("failed to render agent TOML config")?;

    if let Some(path) = output {
        write_secret_file(&path, rendered.as_bytes())?;
    } else {
        print!("{rendered}");
    }
    Ok(())
}

fn claim_enrollment(
    api_url: &str,
    enrollment_token: String,
    client_public_key_hex: String,
) -> Result<ClaimEnrollmentResponse> {
    let body = serde_json::json!({
        "token": enrollment_token,
        "client_public_key_hex": client_public_key_hex,
    });
    let response_body = http_post_json(api_url, "/api/v1/enrollments/claim", None, &body)?;
    serde_json::from_str(&response_body).context("failed to parse enrollment claim response")
}

fn render_agent_config(
    response: &ClaimEnrollmentResponse,
    client_private_key_hex: String,
    command_timeout_secs: u64,
) -> Result<AgentConfig> {
    if response.noise_mode == AgentNoiseMode::EnrolledIk
        && response.gateway_server_public_key_hex.is_none()
    {
        anyhow::bail!("enrollment response is missing gateway server public key for enrolled_ik");
    }
    Ok(AgentConfig {
        client_id: response.client_id.clone(),
        // Keep panel aliases server-side. The agent only needs its opaque id.
        display_name: response.client_id.clone(),
        tcp_endpoints: response.tcp_endpoints.clone(),
        discovery_url: response.discovery_url.clone(),
        noise: AgentNoiseConfig {
            mode: response.noise_mode,
            client_private_key_hex: Some(client_private_key_hex),
            server_public_key_hex: response.gateway_server_public_key_hex.clone(),
        },
        auth: AgentAuthConfig {
            server_ed25519_public_key_hex: response.server_ed25519_public_key_hex.clone(),
            discovery_trusted_server_ed25519_public_keys_hex: response
                .discovery_trusted_server_ed25519_public_keys_hex
                .clone(),
            command_timeout_secs: command_timeout_secs.max(1),
            gateway_retry_secs: response.gateway_retry_secs.max(1),
            gateway_connect_timeout_secs: response.gateway_connect_timeout_secs.max(1),
        },
        backup: AgentBackupConfig::default(),
        update: response.update.clone(),
        execution: AgentExecutionConfig::default(),
        telemetry: AgentTelemetryConfig::default(),
        network: AgentNetworkConfig::default(),
        telemetry_light_secs: response.telemetry_light_secs.max(5),
        telemetry_full_secs: response.telemetry_full_secs.max(5),
        // Keep panel aliases and tags server-side. The agent only needs its opaque id.
        tags: Vec::new(),
    })
}

fn write_secret_file(path: &PathBuf, data: &[u8]) -> Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .with_context(|| format!("failed to open {}", path.display()))?;
    file.write_all(data)
        .with_context(|| format!("failed to write {}", path.display()))
}

#[derive(Clone, Debug, Deserialize)]
struct ClaimEnrollmentResponse {
    client_id: String,
    tcp_endpoints: Vec<ServerEndpoint>,
    discovery_url: Option<String>,
    noise_mode: AgentNoiseMode,
    gateway_server_public_key_hex: Option<String>,
    server_ed25519_public_key_hex: Option<String>,
    #[serde(default)]
    discovery_trusted_server_ed25519_public_keys_hex: Vec<String>,
    gateway_retry_secs: u64,
    gateway_connect_timeout_secs: u64,
    telemetry_light_secs: u64,
    telemetry_full_secs: u64,
    #[serde(default)]
    update: AgentUpdateConfig,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enrollment_settings_update_removes_read_only_server_signing_key() {
        let normalized = normalize_enrollment_settings_update_value(serde_json::json!({
            "server_ed25519_public_key_hex": "11".repeat(32),
            "gateway_retry_secs": 60,
            "gateway_connect_timeout_secs": 10,
            "tcp_endpoints": [{
                "label": "primary",
                "tcp_addr": "gw.ops.example.com:9443",
                "priority": 10,
            }],
        }));

        assert!(normalized.get("server_ed25519_public_key_hex").is_none());
        assert_eq!(normalized["gateway_retry_secs"], 60);
        assert_eq!(
            normalized["tcp_endpoints"][0]["tcp_addr"],
            "gw.ops.example.com:9443"
        );
    }

    #[test]
    fn renders_enrolled_agent_config_without_server_side_private_key() {
        let response: ClaimEnrollmentResponse = serde_json::from_value(serde_json::json!({
            "client_id": "client-a",
            "tcp_endpoints": [{
                "label": "primary",
                "tcp_addr": "198.51.100.10:9443",
                "priority": 10,
            }],
            "discovery_url": "https://panel.example/.well-known/vpsman/endpoints.json",
            "noise_mode": "enrolled_ik",
            "gateway_server_public_key_hex": "11".repeat(32),
            "server_ed25519_public_key_hex": "22".repeat(32),
            "discovery_trusted_server_ed25519_public_keys_hex": ["55".repeat(32)],
            "gateway_retry_secs": 60,
            "gateway_connect_timeout_secs": 10,
            "telemetry_light_secs": 15,
            "telemetry_full_secs": 60,
        }))
        .unwrap();
        let config = render_agent_config(&response, "33".repeat(32), 45).unwrap();
        let rendered = toml::to_string(&config).unwrap();
        let client_private_key_hex = "33".repeat(32);
        let gateway_public_key_hex = "11".repeat(32);

        assert_eq!(config.noise.mode, AgentNoiseMode::EnrolledIk);
        assert_eq!(config.display_name, "client-a");
        assert_eq!(
            config.noise.client_private_key_hex.as_deref(),
            Some(client_private_key_hex.as_str())
        );
        assert_eq!(
            config.noise.server_public_key_hex.as_deref(),
            Some(gateway_public_key_hex.as_str())
        );
        assert_eq!(
            config.auth.discovery_trusted_server_ed25519_public_keys_hex,
            vec!["55".repeat(32)]
        );
        assert_eq!(config.auth.command_timeout_secs, 45);
        assert_eq!(config.auth.gateway_retry_secs, 60);
        assert_eq!(config.auth.gateway_connect_timeout_secs, 10);
        assert!(config.tags.is_empty());
        assert!(rendered.contains("client_private_key_hex"));
        assert!(!rendered.contains("enrollment_token"));
    }

    #[test]
    fn refuses_enrolled_config_without_gateway_public_key() {
        let response = ClaimEnrollmentResponse {
            client_id: "client-a".to_string(),
            tcp_endpoints: Vec::new(),
            discovery_url: None,
            noise_mode: AgentNoiseMode::EnrolledIk,
            gateway_server_public_key_hex: None,
            server_ed25519_public_key_hex: None,
            discovery_trusted_server_ed25519_public_keys_hex: Vec::new(),
            gateway_retry_secs: 60,
            gateway_connect_timeout_secs: 10,
            telemetry_light_secs: 15,
            telemetry_full_secs: 60,
            update: AgentUpdateConfig::default(),
        };

        assert!(render_agent_config(&response, "33".repeat(32), 30)
            .unwrap_err()
            .to_string()
            .contains("missing gateway server public key"));
    }
}
