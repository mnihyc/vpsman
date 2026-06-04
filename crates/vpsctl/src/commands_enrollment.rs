use std::{fs::OpenOptions, io::Write, path::PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;
use vpsman_common::{
    derive_super_key, generate_noise_keypair, AgentAuthConfig, AgentBackupConfig, AgentConfig,
    AgentExecutionConfig, AgentNetworkConfig, AgentNoiseConfig, AgentNoiseMode,
    AgentTelemetryConfig, AgentUpdateConfig, ServerEndpoint,
};

use crate::http::{http_get, http_post_json};
use crate::proof::{load_super_password, load_super_salt_hex};

pub(crate) fn enrollment_tokens(api_url: &str, token: Option<&str>) -> Result<()> {
    println!("{}", http_get(api_url, "/api/v1/enrollment-tokens", token)?);
    Ok(())
}

pub(crate) struct EnrollmentTokenCreateOptions {
    pub(crate) ttl_secs: u64,
    pub(crate) purpose: String,
    pub(crate) allowed_client_id: Option<String>,
    pub(crate) confirmed_reenrollment: bool,
    pub(crate) preserve_existing_assignments: bool,
    pub(crate) default_tags: Vec<String>,
    pub(crate) default_pool_name: Option<String>,
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
                "purpose": options.purpose,
                "allowed_client_id": options.allowed_client_id,
                "confirmed_reenrollment": options.confirmed_reenrollment,
                "preserve_existing_assignments": options.preserve_existing_assignments,
                "default_tags": options.default_tags,
                "default_pool_name": options.default_pool_name,
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
    pub(crate) default_pool_name: Option<String>,
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
                "default_pool_name": options.default_pool_name,
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
    client_id: String,
    client_public_key_hex: String,
) -> Result<()> {
    println!(
        "{}",
        http_post_json(
            api_url,
            "/api/v1/enrollments/claim",
            None,
            &serde_json::json!({
                "token": enrollment_token,
                "client_id": client_id,
                "client_public_key_hex": client_public_key_hex,
            }),
        )?
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn enroll_config(
    api_url: &str,
    enrollment_token: String,
    client_id: String,
    password_env: String,
    super_salt_hex: Option<String>,
    command_timeout_secs: u64,
    output: Option<PathBuf>,
) -> Result<()> {
    let keypair = generate_noise_keypair()?;
    let response = claim_enrollment(api_url, enrollment_token, client_id, keypair.public_hex())?;
    let password = load_super_password(&password_env)?;
    let salt_hex = load_super_salt_hex(super_salt_hex.as_deref())?;
    let salt = hex::decode(&salt_hex).context("super-password salt is not valid hex")?;
    let proof_key_hex = hex::encode(derive_super_key(&password, &salt));
    let config = render_agent_config(
        &response,
        keypair.private_hex(),
        proof_key_hex,
        command_timeout_secs,
    )?;
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
    client_id: String,
    client_public_key_hex: String,
) -> Result<ClaimEnrollmentResponse> {
    let body = http_post_json(
        api_url,
        "/api/v1/enrollments/claim",
        None,
        &serde_json::json!({
            "token": enrollment_token,
            "client_id": client_id,
            "client_public_key_hex": client_public_key_hex,
        }),
    )?;
    serde_json::from_str(&body).context("failed to parse enrollment claim response")
}

fn render_agent_config(
    response: &ClaimEnrollmentResponse,
    client_private_key_hex: String,
    proof_key_hex: String,
    command_timeout_secs: u64,
) -> Result<AgentConfig> {
    if response.noise_mode == AgentNoiseMode::EnrolledIk
        && response.gateway_server_public_key_hex.is_none()
    {
        anyhow::bail!("enrollment response is missing gateway server public key for enrolled_ik");
    }
    Ok(AgentConfig {
        client_id: response.client_id.clone(),
        display_name: response.display_name.clone(),
        tcp_endpoints: response.tcp_endpoints.clone(),
        discovery_url: response.discovery_url.clone(),
        noise: AgentNoiseConfig {
            mode: response.noise_mode,
            client_private_key_hex: Some(client_private_key_hex),
            server_public_key_hex: response.gateway_server_public_key_hex.clone(),
        },
        auth: AgentAuthConfig {
            proof_key_hex: Some(proof_key_hex),
            server_ed25519_public_key_hex: response.server_ed25519_public_key_hex.clone(),
            discovery_trusted_server_ed25519_public_keys_hex: response
                .discovery_trusted_server_ed25519_public_keys_hex
                .clone(),
            command_timeout_secs: command_timeout_secs.max(1),
        },
        backup: AgentBackupConfig::default(),
        update: response.update.clone(),
        execution: AgentExecutionConfig::default(),
        telemetry: AgentTelemetryConfig::default(),
        network: AgentNetworkConfig::default(),
        telemetry_light_secs: response.telemetry_light_secs.max(5),
        telemetry_full_secs: response.telemetry_full_secs.max(5),
        tags: response.tags.clone(),
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
    display_name: String,
    tcp_endpoints: Vec<ServerEndpoint>,
    discovery_url: Option<String>,
    noise_mode: AgentNoiseMode,
    gateway_server_public_key_hex: Option<String>,
    server_ed25519_public_key_hex: Option<String>,
    #[serde(default)]
    discovery_trusted_server_ed25519_public_keys_hex: Vec<String>,
    telemetry_light_secs: u64,
    telemetry_full_secs: u64,
    tags: Vec<String>,
    #[serde(default)]
    update: AgentUpdateConfig,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_enrolled_agent_config_without_server_side_private_key() {
        let response = ClaimEnrollmentResponse {
            client_id: "client-a".to_string(),
            display_name: "client-a".to_string(),
            tcp_endpoints: vec![ServerEndpoint {
                label: "primary".to_string(),
                tcp_addr: "198.51.100.10:9443".to_string(),
                priority: 10,
            }],
            discovery_url: Some(
                "https://panel.example/.well-known/vpsman/endpoints.json".to_string(),
            ),
            noise_mode: AgentNoiseMode::EnrolledIk,
            gateway_server_public_key_hex: Some("11".repeat(32)),
            server_ed25519_public_key_hex: Some("22".repeat(32)),
            discovery_trusted_server_ed25519_public_keys_hex: vec!["55".repeat(32)],
            telemetry_light_secs: 15,
            telemetry_full_secs: 60,
            tags: vec!["edge".to_string()],
            update: AgentUpdateConfig::default(),
        };
        let config = render_agent_config(&response, "33".repeat(32), "44".repeat(32), 45).unwrap();
        let rendered = toml::to_string(&config).unwrap();
        let client_private_key_hex = "33".repeat(32);
        let gateway_public_key_hex = "11".repeat(32);
        let proof_key_hex = "44".repeat(32);

        assert_eq!(config.noise.mode, AgentNoiseMode::EnrolledIk);
        assert_eq!(
            config.noise.client_private_key_hex.as_deref(),
            Some(client_private_key_hex.as_str())
        );
        assert_eq!(
            config.noise.server_public_key_hex.as_deref(),
            Some(gateway_public_key_hex.as_str())
        );
        assert_eq!(
            config.auth.proof_key_hex.as_deref(),
            Some(proof_key_hex.as_str())
        );
        assert_eq!(
            config.auth.discovery_trusted_server_ed25519_public_keys_hex,
            vec!["55".repeat(32)]
        );
        assert_eq!(config.auth.command_timeout_secs, 45);
        assert!(rendered.contains("client_private_key_hex"));
        assert!(!rendered.contains("enrollment_token"));
    }

    #[test]
    fn refuses_enrolled_config_without_gateway_public_key() {
        let response = ClaimEnrollmentResponse {
            client_id: "client-a".to_string(),
            display_name: "client-a".to_string(),
            tcp_endpoints: Vec::new(),
            discovery_url: None,
            noise_mode: AgentNoiseMode::EnrolledIk,
            gateway_server_public_key_hex: None,
            server_ed25519_public_key_hex: None,
            discovery_trusted_server_ed25519_public_keys_hex: Vec::new(),
            telemetry_light_secs: 15,
            telemetry_full_secs: 60,
            tags: Vec::new(),
            update: AgentUpdateConfig::default(),
        };

        assert!(
            render_agent_config(&response, "33".repeat(32), "44".repeat(32), 30)
                .unwrap_err()
                .to_string()
                .contains("missing gateway server public key")
        );
    }
}
