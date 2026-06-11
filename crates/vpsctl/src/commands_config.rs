use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use ed25519_dalek::SigningKey;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;
use vpsman_common::{
    sign_update_artifact_hash, validate_agent_config_shape, verify_update_artifact_signature,
    AgentConfig, JobCommand, MAX_AGENT_HOT_CONFIG_BYTES,
};

use crate::commands_schedules::selector_expression_from_targets;
use crate::http::{http_get, http_post_file, http_post_json};
use crate::jobs::{resolve_target_ids, submit_privileged_operation, PrivilegedOperationRequest};
use crate::privilege::{build_privilege_for_job_command, load_super_password, load_super_salt_hex};
use crate::util::percent_encode_path_segment;

const MAX_UPDATE_ARTIFACT_BYTES: usize = 16 * 1024 * 1024;

pub(crate) struct AgentUpdateOptions {
    pub(crate) artifact_url: String,
    pub(crate) sha256_hex: String,
    pub(crate) artifact_signature_hex: Option<String>,
    pub(crate) artifact_signing_key_hex: Option<String>,
    pub(crate) clients: Vec<String>,
    pub(crate) tags: Vec<String>,
    pub(crate) password_env: String,
    pub(crate) super_salt_hex: Option<String>,
    pub(crate) privilege_ttl_secs: u64,
    pub(crate) timeout_secs: u64,
    pub(crate) confirmed: bool,
    pub(crate) force_unprivileged: bool,
}

pub(crate) struct AgentUpdateCheckOptions {
    pub(crate) version_url: Option<String>,
    pub(crate) activate: bool,
    pub(crate) restart_agent: bool,
    pub(crate) clients: Vec<String>,
    pub(crate) tags: Vec<String>,
    pub(crate) password_env: String,
    pub(crate) super_salt_hex: Option<String>,
    pub(crate) privilege_ttl_secs: u64,
    pub(crate) timeout_secs: u64,
    pub(crate) confirmed: bool,
    pub(crate) force_unprivileged: bool,
}

pub(crate) struct AgentUpdateReleasePublishOptions {
    pub(crate) name: String,
    pub(crate) version: String,
    pub(crate) channel: String,
    pub(crate) artifact_file: PathBuf,
    pub(crate) artifact_url: String,
    pub(crate) signing_seed_hex: String,
    pub(crate) rollback_artifact_file: Option<PathBuf>,
    pub(crate) rollback_artifact_url: Option<String>,
    pub(crate) rollback_signing_seed_hex: Option<String>,
    pub(crate) notes: Option<String>,
    pub(crate) confirmed: bool,
}

#[derive(Debug, Deserialize)]
struct StreamedAgentUpdateArtifactRecord {
    artifact_sha256_hex: String,
    size_bytes: i64,
}

pub(crate) fn hot_config(
    api_url: &str,
    token: Option<&str>,
    config_file: PathBuf,
    clients: Vec<String>,
    tags: Vec<String>,
    password_env: String,
    super_salt_hex: Option<String>,
    privilege_ttl_secs: u64,
    timeout_secs: u64,
    confirmed: bool,
    force_unprivileged: bool,
) -> Result<()> {
    anyhow::ensure!(
        confirmed,
        "hot-config requires --confirmed because it changes persistent agent configuration"
    );
    let toml_document = std::fs::read_to_string(&config_file)
        .with_context(|| format!("failed to read hot config {}", config_file.display()))?;
    anyhow::ensure!(
        toml_document.len() <= MAX_AGENT_HOT_CONFIG_BYTES,
        "hot config exceeds {} bytes",
        MAX_AGENT_HOT_CONFIG_BYTES
    );
    let config: AgentConfig = toml::from_str(&toml_document)
        .with_context(|| format!("failed to parse hot config {}", config_file.display()))?;
    validate_agent_config_shape(&config)
        .map_err(|message| anyhow::anyhow!("invalid hot config: {message}"))?;
    let operation = JobCommand::HotConfig {
        toml: toml_document,
        preserve_redacted: None,
        base_config_sha256_hex: None,
    };
    println!(
        "{}",
        submit_privileged_operation(PrivilegedOperationRequest {
            api_url,
            token,
            operation: &operation,
            command_label: "hot_config",
            clients: &clients,
            tags: &tags,
            password_env: &password_env,
            super_salt_hex: super_salt_hex.as_deref(),
            privilege_ttl_secs,
            timeout_secs,
            confirmed,
            force_unprivileged,
        })?
    );
    Ok(())
}

pub(crate) fn agent_update(
    api_url: &str,
    token: Option<&str>,
    options: AgentUpdateOptions,
) -> Result<()> {
    anyhow::ensure!(
        options.confirmed,
        "agent-update requires --confirmed because it stages a replacement binary"
    );
    validate_update_input(
        &options.artifact_url,
        &options.sha256_hex,
        options.artifact_signature_hex.as_deref(),
        options.artifact_signing_key_hex.as_deref(),
    )?;
    let operation = JobCommand::UpdateAgent {
        artifact_url: options.artifact_url,
        sha256_hex: options.sha256_hex.to_ascii_lowercase(),
        artifact_signature_hex: options
            .artifact_signature_hex
            .map(|value| value.to_ascii_lowercase()),
        artifact_signing_key_hex: options
            .artifact_signing_key_hex
            .map(|value| value.to_ascii_lowercase()),
    };
    let password = load_super_password(&options.password_env)?;
    let salt_hex = load_super_salt_hex(options.super_salt_hex.as_deref())?;
    let selector_expression = selector_expression_from_targets(&options.clients, &options.tags);
    let target_ids = resolve_target_ids(api_url, token, &options.clients, &options.tags)?;
    let privilege = build_privilege_for_job_command(
        &target_ids,
        &operation,
        "agent_update",
        &selector_expression,
        &password,
        &salt_hex,
        options.privilege_ttl_secs,
        options.timeout_secs,
        options.force_unprivileged,
        true,
    )?;
    println!(
        "{}",
        http_post_json(
            api_url,
            "/api/v1/jobs",
            token,
            &serde_json::json!({
                "job_id": Uuid::new_v4(),
                "command": "agent_update",
                "argv": [],
                "operation": operation,
                "selector_expression": selector_expression,
                "target_client_ids": target_ids,
                "privileged": true,
                "destructive": false,
                "confirmed": options.confirmed,
                "force_unprivileged": options.force_unprivileged,
                "timeout_secs": options.timeout_secs,
                "privilege_assertion": privilege.privilege_assertion,
            }),
        )?
    );
    Ok(())
}

pub(crate) fn agent_update_check(
    api_url: &str,
    token: Option<&str>,
    options: AgentUpdateCheckOptions,
) -> Result<()> {
    anyhow::ensure!(
        options.confirmed,
        "agent-update-check requires --confirmed because it may stage and activate a replacement binary"
    );
    if let Some(version_url) = options.version_url.as_deref() {
        anyhow::ensure!(
            version_url.starts_with("https://")
                || version_url.starts_with("http://localhost")
                || version_url.starts_with("http://127.0.0.1")
                || version_url.starts_with("file://"),
            "version URL must use https://, localhost http://, or file://"
        );
    }
    let operation = JobCommand::AgentUpdateCheck {
        version_url: options.version_url,
        activate: options.activate,
        restart_agent: options.restart_agent,
    };
    let password = load_super_password(&options.password_env)?;
    let salt_hex = load_super_salt_hex(options.super_salt_hex.as_deref())?;
    let selector_expression = selector_expression_from_targets(&options.clients, &options.tags);
    let target_ids = resolve_target_ids(api_url, token, &options.clients, &options.tags)?;
    let privilege = build_privilege_for_job_command(
        &target_ids,
        &operation,
        "agent_update_check",
        &selector_expression,
        &password,
        &salt_hex,
        options.privilege_ttl_secs,
        options.timeout_secs,
        options.force_unprivileged,
        true,
    )?;
    println!(
        "{}",
        http_post_json(
            api_url,
            "/api/v1/jobs",
            token,
            &serde_json::json!({
                "job_id": Uuid::new_v4(),
                "command": "agent_update_check",
                "argv": [],
                "operation": operation,
                "selector_expression": selector_expression,
                "target_client_ids": target_ids,
                "privileged": true,
                "destructive": false,
                "confirmed": options.confirmed,
                "force_unprivileged": options.force_unprivileged,
                "timeout_secs": options.timeout_secs,
                "privilege_assertion": privilege.privilege_assertion,
            }),
        )?
    );
    Ok(())
}

pub(crate) fn agent_update_signature(
    artifact_file: PathBuf,
    signing_seed_hex: String,
) -> Result<()> {
    println!(
        "{}",
        build_update_signature_json(&artifact_file, &signing_seed_hex)?
    );
    Ok(())
}

pub(crate) fn agent_update_release_publish(
    api_url: &str,
    token: Option<&str>,
    options: AgentUpdateReleasePublishOptions,
) -> Result<()> {
    anyhow::ensure!(
        options.confirmed,
        "agent-update-release-publish requires --confirmed because it records trusted update metadata"
    );
    let signature = build_update_signature(&options.artifact_file, &options.signing_seed_hex)?;
    validate_update_input(
        &options.artifact_url,
        &signature.artifact_sha256_hex,
        Some(&signature.artifact_signature_hex),
        Some(&signature.artifact_signing_key_hex),
    )?;
    let rollback_signature =
        if let Some(rollback_artifact_file) = options.rollback_artifact_file.as_ref() {
            let rollback_artifact_url = options
                .rollback_artifact_url
                .as_deref()
                .context("--rollback-artifact-url is required with --rollback-artifact-file")?;
            let rollback_signature = build_update_signature(
                rollback_artifact_file,
                options
                    .rollback_signing_seed_hex
                    .as_deref()
                    .unwrap_or(options.signing_seed_hex.as_str()),
            )?;
            validate_update_input(
                rollback_artifact_url,
                &rollback_signature.artifact_sha256_hex,
                Some(&rollback_signature.artifact_signature_hex),
                Some(&rollback_signature.artifact_signing_key_hex),
            )?;
            Some((rollback_artifact_url.to_string(), rollback_signature))
        } else {
            anyhow::ensure!(
            options.rollback_artifact_url.is_none() && options.rollback_signing_seed_hex.is_none(),
            "--rollback-artifact-file is required when rollback URL or rollback signing seed is set"
        );
            None
        };
    println!(
        "{}",
        http_post_json(
            api_url,
            "/api/v1/agent-update-releases",
            token,
            &serde_json::json!({
                "name": options.name,
                "version": options.version,
                "channel": options.channel,
                "artifact_sha256_hex": signature.artifact_sha256_hex,
                "artifact_signature_hex": signature.artifact_signature_hex,
                "artifact_signing_key_hex": signature.artifact_signing_key_hex,
                "artifact_url": options.artifact_url,
                "rollback_artifact_sha256_hex": rollback_signature.as_ref().map(|(_, signature)| signature.artifact_sha256_hex.clone()),
                "rollback_artifact_signature_hex": rollback_signature.as_ref().map(|(_, signature)| signature.artifact_signature_hex.clone()),
                "rollback_artifact_signing_key_hex": rollback_signature.as_ref().map(|(_, signature)| signature.artifact_signing_key_hex.clone()),
                "rollback_artifact_url": rollback_signature.as_ref().map(|(url, _)| url.clone()),
                "rollback_size_bytes": rollback_signature.as_ref().map(|(_, signature)| signature.size_bytes),
                "size_bytes": signature.size_bytes,
                "notes": options.notes,
                "confirmed": options.confirmed,
            }),
        )?
    );
    Ok(())
}

pub(crate) fn agent_update_artifact_upload(
    api_url: &str,
    token: Option<&str>,
    name: String,
    version: String,
    channel: String,
    artifact_file: PathBuf,
    signing_seed_hex: String,
    rollback_artifact_file: Option<PathBuf>,
    rollback_signing_seed_hex: Option<String>,
    notes: Option<String>,
    confirmed: bool,
    stream: bool,
) -> Result<()> {
    println!(
        "{}",
        agent_update_artifact_upload_response(
            api_url,
            token,
            name,
            version,
            channel,
            artifact_file,
            signing_seed_hex,
            rollback_artifact_file,
            rollback_signing_seed_hex,
            notes,
            confirmed,
            stream,
        )?
    );
    Ok(())
}

pub(crate) fn agent_update_artifact_upload_response(
    api_url: &str,
    token: Option<&str>,
    name: String,
    version: String,
    channel: String,
    artifact_file: PathBuf,
    signing_seed_hex: String,
    rollback_artifact_file: Option<PathBuf>,
    rollback_signing_seed_hex: Option<String>,
    notes: Option<String>,
    confirmed: bool,
    stream: bool,
) -> Result<String> {
    if stream {
        return agent_update_artifact_upload_streamed(
            api_url,
            token,
            name,
            version,
            channel,
            artifact_file,
            signing_seed_hex,
            rollback_artifact_file,
            rollback_signing_seed_hex,
            notes,
            confirmed,
        );
    }
    let payload = build_update_artifact_upload_payload(
        name,
        version,
        channel,
        artifact_file,
        signing_seed_hex,
        rollback_artifact_file,
        rollback_signing_seed_hex,
        notes,
        confirmed,
    )?;
    http_post_json(
        api_url,
        "/api/v1/agent-update-releases/upload",
        token,
        &payload,
    )
}

fn agent_update_artifact_upload_streamed(
    api_url: &str,
    token: Option<&str>,
    name: String,
    version: String,
    channel: String,
    artifact_file: PathBuf,
    signing_seed_hex: String,
    rollback_artifact_file: Option<PathBuf>,
    rollback_signing_seed_hex: Option<String>,
    notes: Option<String>,
    confirmed: bool,
) -> Result<String> {
    anyhow::ensure!(
        confirmed,
        "agent-update-artifact-upload --stream requires --confirmed because it uploads trusted update bytes"
    );
    let primary_signature = build_update_signature(&artifact_file, &signing_seed_hex)?;
    let primary_streamed = stream_update_artifact(
        api_url,
        token,
        &artifact_file,
        &primary_signature.artifact_signature_hex,
        &primary_signature.artifact_signing_key_hex,
        confirmed,
    )?;
    anyhow::ensure!(
        primary_streamed.artifact_sha256_hex == primary_signature.artifact_sha256_hex,
        "streamed primary artifact hash did not match local signature hash"
    );
    anyhow::ensure!(
        u64::try_from(primary_streamed.size_bytes).unwrap_or(0) == primary_signature.size_bytes,
        "streamed primary artifact size did not match local file size"
    );
    let rollback = if let Some(rollback_artifact_file) = rollback_artifact_file.as_ref() {
        let rollback_signature = build_update_signature(
            rollback_artifact_file,
            rollback_signing_seed_hex
                .as_deref()
                .unwrap_or(signing_seed_hex.as_str()),
        )?;
        let rollback_streamed = stream_update_artifact(
            api_url,
            token,
            rollback_artifact_file,
            &rollback_signature.artifact_signature_hex,
            &rollback_signature.artifact_signing_key_hex,
            confirmed,
        )?;
        anyhow::ensure!(
            rollback_streamed.artifact_sha256_hex == rollback_signature.artifact_sha256_hex,
            "streamed rollback artifact hash did not match local signature hash"
        );
        anyhow::ensure!(
            u64::try_from(rollback_streamed.size_bytes).unwrap_or(0)
                == rollback_signature.size_bytes,
            "streamed rollback artifact size did not match local file size"
        );
        Some((rollback_signature, rollback_streamed))
    } else {
        anyhow::ensure!(
            rollback_signing_seed_hex.is_none(),
            "--rollback-artifact-file is required when rollback signing seed is set"
        );
        None
    };
    http_post_json(
        api_url,
        "/api/v1/agent-update-releases/hosted",
        token,
        &serde_json::json!({
            "name": name,
            "version": version,
            "channel": channel,
            "artifact_sha256_hex": primary_streamed.artifact_sha256_hex,
            "artifact_signature_hex": primary_signature.artifact_signature_hex,
            "artifact_signing_key_hex": primary_signature.artifact_signing_key_hex,
            "rollback_artifact_sha256_hex": rollback.as_ref().map(|(_, streamed)| streamed.artifact_sha256_hex.clone()),
            "rollback_artifact_signature_hex": rollback.as_ref().map(|(signature, _)| signature.artifact_signature_hex.clone()),
            "rollback_artifact_signing_key_hex": rollback.as_ref().map(|(signature, _)| signature.artifact_signing_key_hex.clone()),
            "notes": notes,
            "confirmed": confirmed,
        }),
    )
}

fn stream_update_artifact(
    api_url: &str,
    token: Option<&str>,
    artifact_file: &Path,
    artifact_signature_hex: &str,
    artifact_signing_key_hex: &str,
    confirmed: bool,
) -> Result<StreamedAgentUpdateArtifactRecord> {
    let body = http_post_file(
        api_url,
        "/api/v1/agent-update-artifacts/stream",
        token,
        artifact_file,
        "application/octet-stream",
        &[
            (
                "x-vpsman-artifact-signature-hex",
                artifact_signature_hex.to_string(),
            ),
            (
                "x-vpsman-artifact-signing-key-hex",
                artifact_signing_key_hex.to_string(),
            ),
            ("x-vpsman-confirmed", confirmed.to_string()),
        ],
    )?;
    serde_json::from_str(&body).context("failed to parse streamed update artifact response")
}

pub(crate) fn agent_update_releases(api_url: &str, token: Option<&str>, limit: u16) -> Result<()> {
    println!(
        "{}",
        http_get(
            api_url,
            &format!(
                "/api/v1/agent-update-releases?limit={}",
                limit.clamp(1, 200)
            ),
            token,
        )?
    );
    Ok(())
}

pub(crate) fn agent_update_release_latest(
    api_url: &str,
    token: Option<&str>,
    name: String,
    channel: String,
) -> Result<()> {
    println!(
        "{}",
        http_get(
            api_url,
            &format!(
                "/api/v1/agent-update-releases/latest?name={}&channel={}",
                percent_encode_path_segment(&name),
                percent_encode_path_segment(&channel)
            ),
            token,
        )?
    );
    Ok(())
}

pub(crate) fn agent_update_activate(
    api_url: &str,
    token: Option<&str>,
    staged_sha256_hex: String,
    clients: Vec<String>,
    tags: Vec<String>,
    password_env: String,
    super_salt_hex: Option<String>,
    privilege_ttl_secs: u64,
    timeout_secs: u64,
    restart_agent: bool,
    confirmed: bool,
    force_unprivileged: bool,
) -> Result<()> {
    anyhow::ensure!(
        confirmed,
        "agent-update-activate requires --confirmed because it replaces the active agent binary"
    );
    let staged_sha256_hex = validate_sha256_arg(&staged_sha256_hex, "--staged-sha256-hex")?;
    let operation = JobCommand::AgentUpdateActivate {
        staged_sha256_hex,
        restart_agent,
    };
    println!(
        "{}",
        submit_privileged_operation(PrivilegedOperationRequest {
            api_url,
            token,
            operation: &operation,
            command_label: "agent_update_activate",
            clients: &clients,
            tags: &tags,
            password_env: &password_env,
            super_salt_hex: super_salt_hex.as_deref(),
            privilege_ttl_secs,
            timeout_secs,
            confirmed,
            force_unprivileged,
        })?
    );
    Ok(())
}

pub(crate) fn agent_update_rollback(
    api_url: &str,
    token: Option<&str>,
    rollback_sha256_hex: Option<String>,
    clients: Vec<String>,
    tags: Vec<String>,
    password_env: String,
    super_salt_hex: Option<String>,
    privilege_ttl_secs: u64,
    timeout_secs: u64,
    confirmed: bool,
    force_unprivileged: bool,
) -> Result<()> {
    anyhow::ensure!(
        confirmed,
        "agent-update-rollback requires --confirmed because it replaces the active agent binary"
    );
    let rollback_sha256_hex = rollback_sha256_hex
        .as_deref()
        .map(|value| validate_sha256_arg(value, "--rollback-sha256-hex"))
        .transpose()?;
    let operation = JobCommand::AgentUpdateRollback {
        rollback_sha256_hex,
    };
    println!(
        "{}",
        submit_privileged_operation(PrivilegedOperationRequest {
            api_url,
            token,
            operation: &operation,
            command_label: "agent_update_rollback",
            clients: &clients,
            tags: &tags,
            password_env: &password_env,
            super_salt_hex: super_salt_hex.as_deref(),
            privilege_ttl_secs,
            timeout_secs,
            confirmed,
            force_unprivileged,
        })?
    );
    Ok(())
}

pub(crate) struct UpdateSignatureMaterial {
    pub(crate) artifact_sha256_hex: String,
    pub(crate) artifact_signature_hex: String,
    pub(crate) artifact_signing_key_hex: String,
    pub(crate) size_bytes: u64,
}

fn build_update_signature_json(
    artifact_file: &PathBuf,
    signing_seed_hex: &str,
) -> Result<serde_json::Value> {
    let signature = build_update_signature(artifact_file, signing_seed_hex)?;
    Ok(serde_json::json!({
            "artifact_sha256_hex": signature.artifact_sha256_hex,
            "artifact_signature_hex": signature.artifact_signature_hex,
            "artifact_signing_key_hex": signature.artifact_signing_key_hex,
    }))
}

pub(crate) fn build_update_artifact_upload_payload(
    name: String,
    version: String,
    channel: String,
    artifact_file: PathBuf,
    signing_seed_hex: String,
    rollback_artifact_file: Option<PathBuf>,
    rollback_signing_seed_hex: Option<String>,
    notes: Option<String>,
    confirmed: bool,
) -> Result<serde_json::Value> {
    anyhow::ensure!(
        confirmed,
        "agent-update-artifact-upload requires --confirmed because it uploads trusted update bytes"
    );
    let artifact = read_update_artifact(&artifact_file)?;
    let signature = build_update_signature_from_bytes(&artifact, &signing_seed_hex)?;
    let rollback = if let Some(rollback_artifact_file) = rollback_artifact_file.as_ref() {
        let rollback_artifact = read_update_artifact(rollback_artifact_file)?;
        let rollback_signature = build_update_signature_from_bytes(
            &rollback_artifact,
            rollback_signing_seed_hex
                .as_deref()
                .unwrap_or(signing_seed_hex.as_str()),
        )?;
        Some((rollback_artifact, rollback_signature))
    } else {
        anyhow::ensure!(
            rollback_signing_seed_hex.is_none(),
            "--rollback-artifact-file is required with --rollback-signing-seed-hex"
        );
        None
    };
    Ok(serde_json::json!({
        "name": name,
        "version": version,
        "channel": channel,
        "artifact_base64": BASE64_STANDARD.encode(&artifact),
        "artifact_signature_hex": signature.artifact_signature_hex,
        "artifact_signing_key_hex": signature.artifact_signing_key_hex,
        "rollback_artifact_base64": rollback.as_ref().map(|(artifact, _)| BASE64_STANDARD.encode(artifact)),
        "rollback_artifact_signature_hex": rollback.as_ref().map(|(_, signature)| signature.artifact_signature_hex.clone()),
        "rollback_artifact_signing_key_hex": rollback.as_ref().map(|(_, signature)| signature.artifact_signing_key_hex.clone()),
        "notes": notes,
        "confirmed": confirmed,
    }))
}

pub(crate) fn build_update_signature(
    artifact_file: &PathBuf,
    signing_seed_hex: &str,
) -> Result<UpdateSignatureMaterial> {
    let artifact = read_update_artifact(artifact_file)?;
    build_update_signature_from_bytes(&artifact, signing_seed_hex)
}

fn build_update_signature_from_bytes(
    artifact: &[u8],
    signing_seed_hex: &str,
) -> Result<UpdateSignatureMaterial> {
    let sha256_hex = hex::encode(Sha256::digest(artifact));
    let signing_seed = decode_fixed_hex(signing_seed_hex, 32, "signing seed")?;
    let signing_key = SigningKey::from_bytes(&signing_seed);
    let signature_hex = hex::encode(sign_update_artifact_hash(&signing_key, &sha256_hex));
    let signing_key_hex = hex::encode(signing_key.verifying_key().to_bytes());
    Ok(UpdateSignatureMaterial {
        artifact_sha256_hex: sha256_hex,
        artifact_signature_hex: signature_hex,
        artifact_signing_key_hex: signing_key_hex,
        size_bytes: artifact.len() as u64,
    })
}

fn read_update_artifact(artifact_file: &PathBuf) -> Result<Vec<u8>> {
    let artifact = std::fs::read(artifact_file)
        .with_context(|| format!("failed to read update artifact {}", artifact_file.display()))?;
    anyhow::ensure!(!artifact.is_empty(), "update artifact file is empty");
    anyhow::ensure!(
        artifact.len() <= MAX_UPDATE_ARTIFACT_BYTES,
        "update artifact exceeds {} bytes",
        MAX_UPDATE_ARTIFACT_BYTES
    );
    Ok(artifact)
}

fn validate_sha256_arg(value: &str, label: &str) -> Result<String> {
    let value = value.trim().to_ascii_lowercase();
    anyhow::ensure!(
        value.len() == 64 && value.as_bytes().iter().all(u8::is_ascii_hexdigit),
        "agent update {label} must be 64 hex characters"
    );
    Ok(value)
}

pub(crate) fn validate_update_input(
    artifact_url: &str,
    sha256_hex: &str,
    artifact_signature_hex: Option<&str>,
    artifact_signing_key_hex: Option<&str>,
) -> Result<()> {
    anyhow::ensure!(
        artifact_url.starts_with("https://"),
        "agent update artifact URL must use https://"
    );
    anyhow::ensure!(
        sha256_hex.len() == 64 && sha256_hex.as_bytes().iter().all(u8::is_ascii_hexdigit),
        "agent update --sha256-hex must be 64 hex characters"
    );
    match (artifact_signature_hex, artifact_signing_key_hex) {
        (Some(signature), Some(signing_key)) => {
            anyhow::ensure!(
                is_hex_len(signature, 128),
                "agent update --artifact-signature-hex must be 128 hex characters"
            );
            anyhow::ensure!(
                is_hex_len(signing_key, 64),
                "agent update --artifact-signing-key-hex must be 64 hex characters"
            );
            anyhow::ensure!(
                verify_update_artifact_signature(
                    signing_key,
                    signature,
                    &sha256_hex.to_ascii_lowercase()
                ),
                "agent update artifact signature does not verify sha256"
            );
        }
        (None, None) => {}
        _ => anyhow::bail!(
            "agent update artifact signature and signing key must be provided together"
        ),
    }
    Ok(())
}

fn is_hex_len(value: &str, expected_len: usize) -> bool {
    value.len() == expected_len && value.as_bytes().iter().all(u8::is_ascii_hexdigit)
}

fn decode_fixed_hex(value: &str, byte_len: usize, label: &str) -> Result<[u8; 32]> {
    anyhow::ensure!(
        byte_len == 32,
        "decode_fixed_hex currently supports 32-byte keys"
    );
    let value = value.trim();
    anyhow::ensure!(
        is_hex_len(value, byte_len * 2),
        "{label} must be {byte_len} byte hex"
    );
    let bytes = hex::decode(value).with_context(|| format!("{label} is not valid hex"))?;
    let bytes: [u8; 32] = bytes
        .as_slice()
        .try_into()
        .map_err(|_| anyhow::anyhow!("{label} must be {byte_len} bytes"))?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::{
        build_update_artifact_upload_payload, build_update_signature_json, validate_update_input,
    };
    use ed25519_dalek::SigningKey;
    use vpsman_common::sign_update_artifact_hash;

    #[test]
    fn validates_signed_update_input() {
        let signing_key = SigningKey::from_bytes(&[45_u8; 32]);
        let sha256_hex = "ab".repeat(32);
        let signature_hex = hex::encode(sign_update_artifact_hash(&signing_key, &sha256_hex));
        let signing_key_hex = hex::encode(signing_key.verifying_key().to_bytes());

        validate_update_input(
            "https://updates.example/vpsman-agent",
            &sha256_hex,
            Some(&signature_hex),
            Some(&signing_key_hex),
        )
        .unwrap();
        assert!(validate_update_input(
            "https://updates.example/vpsman-agent",
            &"cd".repeat(32),
            Some(&signature_hex),
            Some(&signing_key_hex),
        )
        .is_err());
        assert!(validate_update_input(
            "https://updates.example/vpsman-agent",
            &sha256_hex,
            Some(&signature_hex),
            None,
        )
        .is_err());
    }

    #[test]
    fn writes_update_signature_json() {
        let dir =
            std::env::temp_dir().join(format!("vpsman-update-signature-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let artifact = dir.join("vpsman-agent");
        std::fs::write(&artifact, b"agent").unwrap();

        let value = build_update_signature_json(&artifact, &"11".repeat(32)).unwrap();
        assert!(value["artifact_sha256_hex"].as_str().is_some());
        assert!(value["artifact_signature_hex"].as_str().is_some());
        assert!(value["artifact_signing_key_hex"].as_str().is_some());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn builds_hosted_update_artifact_upload_payload() {
        let dir =
            std::env::temp_dir().join(format!("vpsman-update-upload-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let artifact = dir.join("vpsman-agent");
        let rollback = dir.join("vpsman-agent.rollback");
        std::fs::write(&artifact, b"agent").unwrap();
        std::fs::write(&rollback, b"old-agent").unwrap();

        let value = build_update_artifact_upload_payload(
            "vpsman-agent".to_string(),
            "1.0.0".to_string(),
            "stable".to_string(),
            artifact,
            "11".repeat(32),
            Some(rollback),
            Some("22".repeat(32)),
            Some("hosted".to_string()),
            true,
        )
        .unwrap();

        assert_eq!(value["name"], "vpsman-agent");
        assert_eq!(value["version"], "1.0.0");
        assert_eq!(value["channel"], "stable");
        assert_eq!(value["artifact_base64"], "YWdlbnQ=");
        assert!(value["artifact_signature_hex"].as_str().is_some());
        assert!(value["artifact_signing_key_hex"].as_str().is_some());
        assert_eq!(value["rollback_artifact_base64"], "b2xkLWFnZW50");
        assert!(value["rollback_artifact_signature_hex"].as_str().is_some());
        assert!(value["rollback_artifact_signing_key_hex"]
            .as_str()
            .is_some());
        let _ = std::fs::remove_dir_all(dir);
    }
}
