use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use ed25519_dalek::SigningKey;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use vpsman_common::{
    derive_super_key, sign_update_artifact_hash, validate_agent_config_shape,
    verify_update_artifact_signature, AgentConfig, JobCommand, MAX_AGENT_HOT_CONFIG_BYTES,
};

use crate::http::{http_get, http_post_file, http_post_json};
use crate::jobs::{resolve_target_ids, submit_privileged_operation, PrivilegedOperationRequest};
use crate::proof::{build_envelopes_for_job_command, load_super_password, load_super_salt_hex};
use crate::util::percent_encode_path_segment;

const MAX_UPDATE_ARTIFACT_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Deserialize)]
struct AgentUpdateRolloutRecord {
    id: String,
    artifact_sha256_hex: String,
    canary_count: i32,
    #[serde(default)]
    automation_targets: Vec<String>,
    targets: Vec<AgentUpdateRolloutTargetRecord>,
}

#[derive(Debug, Deserialize)]
struct AgentUpdateRolloutTargetRecord {
    client_id: String,
    status: String,
}

#[derive(Debug, Deserialize)]
struct StreamedAgentUpdateArtifactRecord {
    artifact_sha256_hex: String,
    size_bytes: i64,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn hot_config(
    api_url: &str,
    token: Option<&str>,
    config_file: PathBuf,
    clients: Vec<String>,
    pools: Vec<String>,
    tags: Vec<String>,
    password_env: String,
    super_salt_hex: Option<String>,
    proof_ttl_secs: u64,
    timeout_secs: u64,
    confirmed: bool,
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
    };
    println!(
        "{}",
        submit_privileged_operation(PrivilegedOperationRequest {
            api_url,
            token,
            operation: &operation,
            command_label: "hot_config",
            clients: &clients,
            pools: &pools,
            tags: &tags,
            password_env: &password_env,
            super_salt_hex: super_salt_hex.as_deref(),
            proof_ttl_secs,
            timeout_secs,
            confirmed,
            force_unprivileged: false,
        })?
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn super_password_rotate(
    api_url: &str,
    token: Option<&str>,
    new_proof_key_hex: Option<String>,
    new_password_env: Option<String>,
    new_super_salt_hex: Option<String>,
    rotation_generation: Option<String>,
    clients: Vec<String>,
    pools: Vec<String>,
    tags: Vec<String>,
    password_env: String,
    super_salt_hex: Option<String>,
    proof_ttl_secs: u64,
    timeout_secs: u64,
    confirmed: bool,
) -> Result<()> {
    anyhow::ensure!(
        confirmed,
        "super-password-rotate requires --confirmed because it changes privileged proof material"
    );
    let new_proof_key_hex = auth_rotation_proof_key_hex(
        new_proof_key_hex.as_deref(),
        new_password_env.as_deref(),
        new_super_salt_hex.as_deref(),
    )?;
    let rotation_generation = normalize_rotation_generation(rotation_generation.as_deref())?;
    let operation = JobCommand::AuthProofKeyRotate {
        new_proof_key_hex,
        rotation_generation,
    };
    println!(
        "{}",
        submit_privileged_operation(PrivilegedOperationRequest {
            api_url,
            token,
            operation: &operation,
            command_label: "auth_proof_key_rotate",
            clients: &clients,
            pools: &pools,
            tags: &tags,
            password_env: &password_env,
            super_salt_hex: super_salt_hex.as_deref(),
            proof_ttl_secs,
            timeout_secs,
            confirmed,
            force_unprivileged: false,
        })?
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn agent_update(
    api_url: &str,
    token: Option<&str>,
    artifact_url: String,
    sha256_hex: String,
    artifact_signature_hex: Option<String>,
    artifact_signing_key_hex: Option<String>,
    clients: Vec<String>,
    pools: Vec<String>,
    tags: Vec<String>,
    password_env: String,
    super_salt_hex: Option<String>,
    proof_ttl_secs: u64,
    timeout_secs: u64,
    canary_count: Option<u16>,
    confirmed: bool,
    force_unprivileged: bool,
) -> Result<()> {
    anyhow::ensure!(
        confirmed,
        "agent-update requires --confirmed because it stages a replacement binary"
    );
    validate_update_input(
        &artifact_url,
        &sha256_hex,
        artifact_signature_hex.as_deref(),
        artifact_signing_key_hex.as_deref(),
    )?;
    let operation = JobCommand::UpdateAgent {
        artifact_url,
        sha256_hex: sha256_hex.to_ascii_lowercase(),
        artifact_signature_hex: artifact_signature_hex.map(|value| value.to_ascii_lowercase()),
        artifact_signing_key_hex: artifact_signing_key_hex.map(|value| value.to_ascii_lowercase()),
    };
    let password = load_super_password(&password_env)?;
    let salt_hex = load_super_salt_hex(super_salt_hex.as_deref())?;
    let target_ids = resolve_target_ids(api_url, token, &clients, &pools, &tags, false, confirmed)?;
    let (_payload_hash_hex, envelopes) = build_envelopes_for_job_command(
        &target_ids,
        &operation,
        &password,
        &salt_hex,
        proof_ttl_secs,
    )?;
    println!(
        "{}",
        http_post_json(
            api_url,
            "/api/v1/jobs",
            token,
            &serde_json::json!({
                "command": "agent_update",
                "argv": [],
                "operation": operation,
                "clients": clients,
                "pools": pools,
                "tags": tags,
                "privileged": true,
                "destructive": false,
                "confirmed": confirmed,
                "force_unprivileged": force_unprivileged,
                "timeout_secs": timeout_secs,
                "canary_count": canary_count,
                "envelope": null,
                "envelopes": envelopes,
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

#[allow(clippy::too_many_arguments)]
pub(crate) fn agent_update_release_publish(
    api_url: &str,
    token: Option<&str>,
    name: String,
    version: String,
    channel: String,
    artifact_file: PathBuf,
    artifact_url: String,
    signing_seed_hex: String,
    rollback_artifact_file: Option<PathBuf>,
    rollback_artifact_url: Option<String>,
    rollback_signing_seed_hex: Option<String>,
    notes: Option<String>,
    confirmed: bool,
) -> Result<()> {
    anyhow::ensure!(
        confirmed,
        "agent-update-release-publish requires --confirmed because it records trusted update metadata"
    );
    let signature = build_update_signature(&artifact_file, &signing_seed_hex)?;
    validate_update_input(
        &artifact_url,
        &signature.artifact_sha256_hex,
        Some(&signature.artifact_signature_hex),
        Some(&signature.artifact_signing_key_hex),
    )?;
    let rollback_signature = if let Some(rollback_artifact_file) = rollback_artifact_file.as_ref() {
        let rollback_artifact_url = rollback_artifact_url
            .as_deref()
            .context("--rollback-artifact-url is required with --rollback-artifact-file")?;
        let rollback_signature = build_update_signature(
            rollback_artifact_file,
            rollback_signing_seed_hex
                .as_deref()
                .unwrap_or(signing_seed_hex.as_str()),
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
            rollback_artifact_url.is_none() && rollback_signing_seed_hex.is_none(),
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
                "name": name,
                "version": version,
                "channel": channel,
                "artifact_sha256_hex": signature.artifact_sha256_hex,
                "artifact_signature_hex": signature.artifact_signature_hex,
                "artifact_signing_key_hex": signature.artifact_signing_key_hex,
                "artifact_url": artifact_url,
                "rollback_artifact_sha256_hex": rollback_signature.as_ref().map(|(_, signature)| signature.artifact_sha256_hex.clone()),
                "rollback_artifact_signature_hex": rollback_signature.as_ref().map(|(_, signature)| signature.artifact_signature_hex.clone()),
                "rollback_artifact_signing_key_hex": rollback_signature.as_ref().map(|(_, signature)| signature.artifact_signing_key_hex.clone()),
                "rollback_artifact_url": rollback_signature.as_ref().map(|(url, _)| url.clone()),
                "rollback_size_bytes": rollback_signature.as_ref().map(|(_, signature)| signature.size_bytes),
                "size_bytes": signature.size_bytes,
                "notes": notes,
                "confirmed": confirmed,
            }),
        )?
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
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

#[allow(clippy::too_many_arguments)]
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

#[allow(clippy::too_many_arguments)]
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

pub(crate) fn agent_update_rollouts(api_url: &str, token: Option<&str>, limit: u16) -> Result<()> {
    println!(
        "{}",
        http_get(
            api_url,
            &format!(
                "/api/v1/agent-update-rollouts?limit={}",
                limit.clamp(1, 200)
            ),
            token,
        )?
    );
    Ok(())
}

pub(crate) fn agent_update_rollout_policies(
    api_url: &str,
    token: Option<&str>,
    limit: u16,
    enabled: Option<bool>,
    channel: Option<String>,
) -> Result<()> {
    let mut path = format!(
        "/api/v1/agent-update-rollout-policies?limit={}",
        limit.clamp(1, 200)
    );
    if let Some(enabled) = enabled {
        path.push_str(&format!("&enabled={enabled}"));
    }
    if let Some(channel) = channel
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        path.push_str("&channel=");
        path.push_str(&percent_encode_path_segment(channel));
    }
    println!("{}", http_get(api_url, &path, token)?);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn agent_update_rollout_policy_create(
    api_url: &str,
    token: Option<&str>,
    name: String,
    scope_kind: String,
    scope_value: Option<String>,
    channel: Option<String>,
    canary_count: Option<i32>,
    health_gate: Option<String>,
    priority: i32,
    enabled: bool,
    notes: Option<String>,
    confirmed: bool,
) -> Result<()> {
    anyhow::ensure!(
        confirmed,
        "agent-update-rollout-policy-create requires --confirmed because it changes reusable rollout defaults"
    );
    validate_rollout_policy_scope(&scope_kind, scope_value.as_deref())?;
    if let Some(canary_count) = canary_count {
        anyhow::ensure!(
            (0..=10_000).contains(&canary_count),
            "--canary-count must be between 0 and 10000"
        );
    }
    if let Some(health_gate) = health_gate.as_deref() {
        validate_rollout_health_gate(health_gate)?;
    }
    println!(
        "{}",
        http_post_json(
            api_url,
            "/api/v1/agent-update-rollout-policies",
            token,
            &serde_json::json!({
                "name": name,
                "scope_kind": scope_kind,
                "scope_value": scope_value,
                "channel": channel,
                "canary_count": canary_count,
                "automation_health_gate": health_gate,
                "priority": priority,
                "enabled": enabled,
                "notes": notes,
                "confirmed": confirmed,
            }),
        )?
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn agent_update_rollout_control(
    api_url: &str,
    token: Option<&str>,
    rollout_id: String,
    pause: bool,
    resume: bool,
    pause_reason: Option<String>,
    automation_health_gate: Option<String>,
    confirmed: bool,
) -> Result<()> {
    anyhow::ensure!(
        confirmed,
        "agent-update-rollout-control requires --confirmed because it changes rollout automation policy"
    );
    anyhow::ensure!(
        !(pause && resume),
        "agent-update-rollout-control cannot use --pause and --resume together"
    );
    let paused = if pause {
        Some(true)
    } else if resume {
        Some(false)
    } else {
        None
    };
    if paused.is_none() && automation_health_gate.is_none() {
        anyhow::bail!("agent-update-rollout-control requires --pause, --resume, or --health-gate");
    }
    if let Some(health_gate) = automation_health_gate.as_deref() {
        validate_rollout_health_gate(health_gate)?;
    }
    println!(
        "{}",
        http_post_json(
            api_url,
            &format!(
                "/api/v1/agent-update-rollouts/{}/control",
                percent_encode_path_segment(&rollout_id)
            ),
            token,
            &serde_json::json!({
                "confirmed": confirmed,
                "paused": paused,
                "pause_reason": pause_reason,
                "automation_health_gate": automation_health_gate,
            }),
        )?
    );
    Ok(())
}

fn load_rollout(
    api_url: &str,
    token: Option<&str>,
    rollout_id: &str,
) -> Result<AgentUpdateRolloutRecord> {
    let body = http_get(api_url, "/api/v1/agent-update-rollouts?limit=200", token)?;
    let rollouts: Vec<AgentUpdateRolloutRecord> =
        serde_json::from_str(&body).context("failed to parse agent update rollout list")?;
    rollouts
        .into_iter()
        .find(|rollout| rollout.id == rollout_id)
        .with_context(|| {
            format!("agent update rollout {rollout_id} was not found in latest 200 records")
        })
}

fn select_rollout_targets(
    rollout: &AgentUpdateRolloutRecord,
    explicit_clients: &[String],
    batch_size: Option<u16>,
    eligible_statuses: &[&str],
    empty_message: &'static str,
) -> Result<Vec<String>> {
    let mut candidates = rollout
        .automation_targets
        .iter()
        .filter(|client_id| {
            rollout.targets.iter().any(|target| {
                target.client_id == **client_id
                    && eligible_statuses.contains(&target.status.as_str())
            })
        })
        .cloned()
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        candidates = rollout
            .targets
            .iter()
            .filter(|target| eligible_statuses.contains(&target.status.as_str()))
            .map(|target| target.client_id.clone())
            .collect();
    }
    if !explicit_clients.is_empty() {
        candidates.retain(|client_id| explicit_clients.iter().any(|wanted| wanted == client_id));
    }
    candidates.sort();
    candidates.dedup();
    let limit = batch_size.map(|value| usize::from(value.max(1)));
    if let Some(limit) = limit {
        candidates.truncate(limit);
    }
    anyhow::ensure!(!candidates.is_empty(), empty_message);
    Ok(candidates)
}

fn select_rollout_delegation_targets(
    rollout: &AgentUpdateRolloutRecord,
    explicit_clients: &[String],
) -> Result<Vec<String>> {
    let rollout_clients = rollout
        .targets
        .iter()
        .map(|target| target.client_id.clone())
        .collect::<std::collections::HashSet<_>>();
    let mut candidates = if explicit_clients.is_empty() {
        let automation_targets = rollout
            .automation_targets
            .iter()
            .filter(|client_id| rollout_clients.contains(*client_id))
            .cloned()
            .collect::<Vec<_>>();
        if automation_targets.is_empty() {
            rollout_clients.iter().cloned().collect()
        } else {
            automation_targets
        }
    } else {
        for client_id in explicit_clients {
            anyhow::ensure!(
                rollout_clients.contains(client_id),
                "--clients contains {client_id}, which is not part of this rollout"
            );
        }
        explicit_clients.to_vec()
    };
    candidates.sort();
    candidates.dedup();
    anyhow::ensure!(
        !candidates.is_empty(),
        "no rollout targets are available for rollback delegation"
    );
    Ok(candidates)
}

fn select_rollout_activation_delegation_targets(
    rollout: &AgentUpdateRolloutRecord,
    explicit_clients: &[String],
) -> Result<Vec<String>> {
    let rollout_clients = rollout
        .targets
        .iter()
        .map(|target| target.client_id.clone())
        .collect::<std::collections::HashSet<_>>();
    let mut candidates = if explicit_clients.is_empty() {
        rollout_clients.iter().cloned().collect()
    } else {
        for client_id in explicit_clients {
            anyhow::ensure!(
                rollout_clients.contains(client_id),
                "--clients contains {client_id}, which is not part of this rollout"
            );
        }
        explicit_clients.to_vec()
    };
    candidates.sort();
    candidates.dedup();
    anyhow::ensure!(
        !candidates.is_empty(),
        "no rollout targets are available for activation delegation"
    );
    Ok(candidates)
}

fn rollout_canary_batch_size(canary_count: i32) -> Option<u16> {
    (canary_count > 0).then_some(canary_count as u16)
}

fn validate_rollout_health_gate(value: &str) -> Result<()> {
    anyhow::ensure!(
        matches!(
            value,
            "heartbeat_verified" | "manual_after_canary" | "manual_only"
        ),
        "--health-gate must be heartbeat_verified, manual_after_canary, or manual_only"
    );
    Ok(())
}

fn validate_rollout_policy_scope(scope_kind: &str, scope_value: Option<&str>) -> Result<()> {
    let scope_kind = scope_kind.trim();
    anyhow::ensure!(
        matches!(scope_kind, "global" | "tag" | "pool" | "provider"),
        "--scope-kind must be global, tag, pool, or provider"
    );
    if scope_kind == "global" {
        anyhow::ensure!(
            scope_value.map(str::trim).unwrap_or("").is_empty(),
            "--scope-value is not allowed for global rollout policies"
        );
    } else {
        anyhow::ensure!(
            !scope_value.map(str::trim).unwrap_or("").is_empty(),
            "--scope-value is required unless --scope-kind global"
        );
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn submit_rollout_operation(
    api_url: &str,
    token: Option<&str>,
    command_label: &str,
    operation: JobCommand,
    clients: Vec<String>,
    password_env: String,
    super_salt_hex: Option<String>,
    proof_ttl_secs: u64,
    timeout_secs: u64,
    force_unprivileged: bool,
    confirmed: bool,
) -> Result<()> {
    let password = load_super_password(&password_env)?;
    let salt_hex = load_super_salt_hex(super_salt_hex.as_deref())?;
    let (_payload_hash_hex, envelopes) = build_envelopes_for_job_command(
        &clients,
        &operation,
        &password,
        &salt_hex,
        proof_ttl_secs,
    )?;
    println!(
        "{}",
        http_post_json(
            api_url,
            "/api/v1/jobs",
            token,
            &serde_json::json!({
                "command": command_label,
                "argv": [],
                "operation": operation,
                "clients": clients,
                "pools": [],
                "tags": [],
                "privileged": true,
                "destructive": false,
                "confirmed": confirmed,
                "force_unprivileged": force_unprivileged,
                "timeout_secs": timeout_secs,
                "envelope": null,
                "envelopes": envelopes,
            }),
        )?
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn agent_update_rollout_activate(
    api_url: &str,
    token: Option<&str>,
    rollout_id: String,
    batch_size: Option<u16>,
    clients: Vec<String>,
    password_env: String,
    super_salt_hex: Option<String>,
    proof_ttl_secs: u64,
    timeout_secs: u64,
    restart_agent: bool,
    force_unprivileged: bool,
    confirmed: bool,
) -> Result<()> {
    anyhow::ensure!(
        confirmed,
        "agent-update-rollout-activate requires --confirmed because it promotes staged binaries"
    );
    let rollout = load_rollout(api_url, token, &rollout_id)?;
    let selected_clients = select_rollout_targets(
        &rollout,
        &clients,
        batch_size.or(rollout_canary_batch_size(rollout.canary_count)),
        &["completed"],
        "no staged rollout targets are eligible for activation",
    )?;
    submit_rollout_operation(
        api_url,
        token,
        "agent_update_activate",
        JobCommand::AgentUpdateActivate {
            staged_sha256_hex: rollout.artifact_sha256_hex,
            restart_agent,
        },
        selected_clients,
        password_env,
        super_salt_hex,
        proof_ttl_secs,
        timeout_secs,
        force_unprivileged,
        confirmed,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn agent_update_rollout_rollback(
    api_url: &str,
    token: Option<&str>,
    rollout_id: String,
    rollback_sha256_hex: Option<String>,
    clients: Vec<String>,
    password_env: String,
    super_salt_hex: Option<String>,
    proof_ttl_secs: u64,
    timeout_secs: u64,
    force_unprivileged: bool,
    confirmed: bool,
) -> Result<()> {
    anyhow::ensure!(
        confirmed,
        "agent-update-rollout-rollback requires --confirmed because it restores rollback binaries"
    );
    let rollout = load_rollout(api_url, token, &rollout_id)?;
    let selected_clients = select_rollout_targets(
        &rollout,
        &clients,
        None,
        &[
            "activation_pending_restart",
            "activation_failed",
            "heartbeat_timeout",
            "heartbeat_verified",
        ],
        "no activation-pending, activation-failed, heartbeat-timeout, or heartbeat-verified rollout targets are eligible for rollback",
    )?;
    let rollback_sha256_hex = rollback_sha256_hex
        .as_deref()
        .map(|value| validate_sha256_arg(value, "--rollback-sha256-hex"))
        .transpose()?;
    submit_rollout_operation(
        api_url,
        token,
        "agent_update_rollback",
        JobCommand::AgentUpdateRollback {
            rollback_sha256_hex,
        },
        selected_clients,
        password_env,
        super_salt_hex,
        proof_ttl_secs,
        timeout_secs,
        force_unprivileged,
        confirmed,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn agent_update_rollout_delegate_rollback(
    api_url: &str,
    token: Option<&str>,
    rollout_id: String,
    rollback_sha256_hex: Option<String>,
    clients: Vec<String>,
    password_env: String,
    super_salt_hex: Option<String>,
    proof_ttl_secs: u64,
    force_unprivileged: bool,
    confirmed: bool,
) -> Result<()> {
    anyhow::ensure!(
        confirmed,
        "agent-update-rollout-delegate-rollback requires --confirmed because it escrows privileged rollback proofs"
    );
    anyhow::ensure!(
        (15..=86_400).contains(&proof_ttl_secs),
        "--proof-ttl-secs must be between 15 and 86400 seconds"
    );
    let rollout = load_rollout(api_url, token, &rollout_id)?;
    let selected_clients = select_rollout_delegation_targets(&rollout, &clients)?;
    let rollback_sha256_hex = rollback_sha256_hex
        .as_deref()
        .map(|value| validate_sha256_arg(value, "--rollback-sha256-hex"))
        .transpose()?;
    let operation = JobCommand::AgentUpdateRollback {
        rollback_sha256_hex: rollback_sha256_hex.clone(),
    };
    let password = load_super_password(&password_env)?;
    let salt_hex = load_super_salt_hex(super_salt_hex.as_deref())?;
    let (_payload_hash_hex, envelopes) = build_envelopes_for_job_command(
        &selected_clients,
        &operation,
        &password,
        &salt_hex,
        proof_ttl_secs,
    )?;
    println!(
        "{}",
        http_post_json(
            api_url,
            &format!(
                "/api/v1/agent-update-rollouts/{}/rollback-delegation",
                percent_encode_path_segment(&rollout_id)
            ),
            token,
            &serde_json::json!({
                "confirmed": confirmed,
                "rollback_sha256_hex": rollback_sha256_hex,
                "force_unprivileged": force_unprivileged,
                "envelopes": envelopes,
            }),
        )?
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn agent_update_rollout_delegate_activation(
    api_url: &str,
    token: Option<&str>,
    rollout_id: String,
    clients: Vec<String>,
    password_env: String,
    super_salt_hex: Option<String>,
    proof_ttl_secs: u64,
    restart_agent: bool,
    force_unprivileged: bool,
    confirmed: bool,
) -> Result<()> {
    anyhow::ensure!(
        confirmed,
        "agent-update-rollout-delegate-activation requires --confirmed because it escrows privileged activation proofs"
    );
    anyhow::ensure!(
        (15..=86_400).contains(&proof_ttl_secs),
        "--proof-ttl-secs must be between 15 and 86400 seconds"
    );
    let rollout = load_rollout(api_url, token, &rollout_id)?;
    let selected_clients = select_rollout_activation_delegation_targets(&rollout, &clients)?;
    let operation = JobCommand::AgentUpdateActivate {
        staged_sha256_hex: rollout.artifact_sha256_hex.clone(),
        restart_agent,
    };
    let password = load_super_password(&password_env)?;
    let salt_hex = load_super_salt_hex(super_salt_hex.as_deref())?;
    let (_payload_hash_hex, envelopes) = build_envelopes_for_job_command(
        &selected_clients,
        &operation,
        &password,
        &salt_hex,
        proof_ttl_secs,
    )?;
    println!(
        "{}",
        http_post_json(
            api_url,
            &format!(
                "/api/v1/agent-update-rollouts/{}/activation-delegation",
                percent_encode_path_segment(&rollout_id)
            ),
            token,
            &serde_json::json!({
                "confirmed": confirmed,
                "restart_agent": restart_agent,
                "force_unprivileged": force_unprivileged,
                "envelopes": envelopes,
            }),
        )?
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn agent_update_activate(
    api_url: &str,
    token: Option<&str>,
    staged_sha256_hex: String,
    clients: Vec<String>,
    pools: Vec<String>,
    tags: Vec<String>,
    password_env: String,
    super_salt_hex: Option<String>,
    proof_ttl_secs: u64,
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
            pools: &pools,
            tags: &tags,
            password_env: &password_env,
            super_salt_hex: super_salt_hex.as_deref(),
            proof_ttl_secs,
            timeout_secs,
            confirmed,
            force_unprivileged,
        })?
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn agent_update_rollback(
    api_url: &str,
    token: Option<&str>,
    rollback_sha256_hex: Option<String>,
    clients: Vec<String>,
    pools: Vec<String>,
    tags: Vec<String>,
    password_env: String,
    super_salt_hex: Option<String>,
    proof_ttl_secs: u64,
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
            pools: &pools,
            tags: &tags,
            password_env: &password_env,
            super_salt_hex: super_salt_hex.as_deref(),
            proof_ttl_secs,
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

#[allow(clippy::too_many_arguments)]
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

pub(crate) fn auth_rotation_proof_key_hex(
    new_proof_key_hex: Option<&str>,
    new_password_env: Option<&str>,
    new_super_salt_hex: Option<&str>,
) -> Result<String> {
    match (new_proof_key_hex, new_password_env) {
        (Some(_), Some(_)) => {
            anyhow::bail!("use either --new-proof-key-hex or --new-password-env, not both")
        }
        (Some(value), None) => validate_hex32_arg(value, "--new-proof-key-hex"),
        (None, Some(password_env)) => {
            let password = load_super_password(password_env)?;
            let salt_hex = match new_super_salt_hex {
                Some(value) => value.to_string(),
                None => std::env::var("VPSMAN_NEW_SUPER_SALT_HEX").context(
                    "set --new-super-salt-hex or VPSMAN_NEW_SUPER_SALT_HEX for local rotation-key derivation",
                )?,
            };
            let salt =
                hex::decode(salt_hex.trim()).context("new super-password salt is not valid hex")?;
            anyhow::ensure!(
                !salt.is_empty(),
                "new super-password salt decodes to empty salt"
            );
            Ok(hex::encode(derive_super_key(&password, &salt)))
        }
        (None, None) => anyhow::bail!(
            "super-password rotation requires --new-proof-key-hex or --new-password-env"
        ),
    }
}

pub(crate) fn normalize_rotation_generation(value: Option<&str>) -> Result<Option<String>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.trim();
    anyhow::ensure!(
        !value.is_empty() && value.len() <= 128 && !value.chars().any(char::is_control),
        "--rotation-generation must be 1-128 non-control characters"
    );
    Ok(Some(value.to_string()))
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

fn validate_hex32_arg(value: &str, label: &str) -> Result<String> {
    let value = value.trim().to_ascii_lowercase();
    anyhow::ensure!(
        value.len() == 64 && value.as_bytes().iter().all(u8::is_ascii_hexdigit),
        "{label} must be 64 hex characters"
    );
    Ok(value)
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
        agent_update_rollout_control, agent_update_rollout_policy_create,
        auth_rotation_proof_key_hex, build_update_artifact_upload_payload,
        build_update_signature_json, normalize_rotation_generation,
        select_rollout_activation_delegation_targets, select_rollout_delegation_targets,
        select_rollout_targets, validate_update_input, AgentUpdateRolloutRecord,
        AgentUpdateRolloutTargetRecord,
    };
    use ed25519_dalek::SigningKey;
    use vpsman_common::{derive_super_key, sign_update_artifact_hash};

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
    fn derives_auth_rotation_key_locally_or_accepts_derived_key() {
        let direct = auth_rotation_proof_key_hex(Some(&"aa".repeat(32)), None, None).unwrap();
        assert_eq!(direct, "aa".repeat(32));

        let env_name = format!("VPSMAN_TEST_NEXT_PASSWORD_{}", uuid::Uuid::new_v4());
        std::env::set_var(&env_name, "next-super-password");
        let derived = auth_rotation_proof_key_hex(None, Some(&env_name), Some("01020304")).unwrap();
        std::env::remove_var(&env_name);
        assert_eq!(
            derived,
            hex::encode(derive_super_key("next-super-password", &[1, 2, 3, 4]))
        );

        assert!(
            auth_rotation_proof_key_hex(Some(&"aa".repeat(32)), Some(&env_name), None).is_err()
        );
        assert!(auth_rotation_proof_key_hex(Some("not-a-key"), None, None).is_err());
    }

    #[test]
    fn validates_auth_rotation_generation_label() {
        assert_eq!(
            normalize_rotation_generation(Some(" 2026-q2 ")).unwrap(),
            Some("2026-q2".to_string())
        );
        assert!(normalize_rotation_generation(Some("")).is_err());
        assert!(normalize_rotation_generation(Some("bad\nlabel")).is_err());
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

    #[test]
    fn rollout_target_selection_prefers_worker_recommendations() {
        let rollout = AgentUpdateRolloutRecord {
            id: "rollout-a".to_string(),
            artifact_sha256_hex: "ab".repeat(32),
            canary_count: 1,
            automation_targets: vec!["client-b".to_string()],
            targets: vec![
                AgentUpdateRolloutTargetRecord {
                    client_id: "client-a".to_string(),
                    status: "completed".to_string(),
                },
                AgentUpdateRolloutTargetRecord {
                    client_id: "client-b".to_string(),
                    status: "completed".to_string(),
                },
            ],
        };

        let selected = select_rollout_targets(
            &rollout,
            &[],
            Some(10),
            &["completed"],
            "no eligible targets",
        )
        .unwrap();

        assert_eq!(selected, vec!["client-b"]);
    }

    #[test]
    fn rollback_delegation_targets_use_automation_scope_and_validate_explicit_clients() {
        let rollout = AgentUpdateRolloutRecord {
            id: "rollout-a".to_string(),
            artifact_sha256_hex: "ab".repeat(32),
            canary_count: 1,
            automation_targets: vec!["client-b".to_string()],
            targets: vec![
                AgentUpdateRolloutTargetRecord {
                    client_id: "client-a".to_string(),
                    status: "completed".to_string(),
                },
                AgentUpdateRolloutTargetRecord {
                    client_id: "client-b".to_string(),
                    status: "activation_pending_restart".to_string(),
                },
            ],
        };

        let selected = select_rollout_delegation_targets(&rollout, &[]).unwrap();
        assert_eq!(selected, vec!["client-b"]);

        let explicit =
            select_rollout_delegation_targets(&rollout, &["client-a".to_string()]).unwrap();
        assert_eq!(explicit, vec!["client-a"]);

        assert!(select_rollout_delegation_targets(&rollout, &["client-c".to_string()]).is_err());
    }

    #[test]
    fn activation_delegation_targets_default_to_full_rollout_scope() {
        let rollout = AgentUpdateRolloutRecord {
            id: "rollout-a".to_string(),
            artifact_sha256_hex: "ab".repeat(32),
            canary_count: 1,
            automation_targets: vec!["client-b".to_string()],
            targets: vec![
                AgentUpdateRolloutTargetRecord {
                    client_id: "client-a".to_string(),
                    status: "completed".to_string(),
                },
                AgentUpdateRolloutTargetRecord {
                    client_id: "client-b".to_string(),
                    status: "completed".to_string(),
                },
            ],
        };

        let selected = select_rollout_activation_delegation_targets(&rollout, &[]).unwrap();
        assert_eq!(selected, vec!["client-a", "client-b"]);

        let explicit =
            select_rollout_activation_delegation_targets(&rollout, &["client-b".to_string()])
                .unwrap();
        assert_eq!(explicit, vec!["client-b"]);

        assert!(
            select_rollout_activation_delegation_targets(&rollout, &["client-c".to_string()])
                .is_err()
        );
    }

    #[test]
    fn rollout_control_validation_fails_before_http_for_bad_requests() {
        assert!(agent_update_rollout_control(
            "http://127.0.0.1:1",
            None,
            "rollout-a".to_string(),
            true,
            true,
            None,
            None,
            true,
        )
        .is_err());
        assert!(agent_update_rollout_control(
            "http://127.0.0.1:1",
            None,
            "rollout-a".to_string(),
            false,
            false,
            None,
            Some("dispatch_without_proof".to_string()),
            true,
        )
        .is_err());
        assert!(agent_update_rollout_control(
            "http://127.0.0.1:1",
            None,
            "rollout-a".to_string(),
            true,
            false,
            None,
            Some("manual_only".to_string()),
            false,
        )
        .is_err());
    }

    #[test]
    fn rollout_policy_create_validation_fails_before_http_for_bad_requests() {
        assert!(agent_update_rollout_policy_create(
            "http://127.0.0.1:1",
            None,
            "stable".to_string(),
            "global".to_string(),
            Some("unexpected".to_string()),
            Some("stable".to_string()),
            Some(1),
            Some("heartbeat_verified".to_string()),
            0,
            true,
            None,
            true,
        )
        .is_err());
        assert!(agent_update_rollout_policy_create(
            "http://127.0.0.1:1",
            None,
            "stable".to_string(),
            "provider".to_string(),
            Some("hetzner".to_string()),
            Some("stable".to_string()),
            Some(10001),
            Some("heartbeat_verified".to_string()),
            0,
            true,
            None,
            true,
        )
        .is_err());
        assert!(agent_update_rollout_policy_create(
            "http://127.0.0.1:1",
            None,
            "stable".to_string(),
            "provider".to_string(),
            Some("hetzner".to_string()),
            Some("stable".to_string()),
            Some(1),
            Some("bad_gate".to_string()),
            0,
            true,
            None,
            true,
        )
        .is_err());
        assert!(agent_update_rollout_policy_create(
            "http://127.0.0.1:1",
            None,
            "stable".to_string(),
            "provider".to_string(),
            Some("hetzner".to_string()),
            Some("stable".to_string()),
            Some(1),
            Some("heartbeat_verified".to_string()),
            0,
            true,
            None,
            false,
        )
        .is_err());
    }
}
