use std::{net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};

mod agent_update_artifact_ingest;
mod auth_model;
mod auth_totp;
mod backup_auto_artifacts;
mod backup_handoff;
mod backup_upload_sessions;
mod data_source_builtin_presets;
mod error;
mod fleet_alert_notifications;
mod fleet_alerts;
mod gateway_client;
mod job_files;
mod job_lifecycle;
mod job_request;
mod job_target_validation;
mod job_terminal;
mod model;
mod model_agent_updates;
mod model_alert_notifications;
mod model_alert_policies;
mod model_alert_states;
mod model_backups;
mod model_command_templates;
mod model_data_sources;
mod model_file_transfer;
mod model_history;
mod model_rollout_policies;
mod model_terminal;
mod model_topology;
mod object_store;
mod repository;
mod repository_agent_update_lifecycle;
mod repository_agent_update_releases;
mod repository_alert_notifications;
mod repository_alert_policies;
mod repository_alert_states;
mod repository_auth;
mod repository_backup_artifacts;
mod repository_backup_policies;
mod repository_backups;
mod repository_command_templates;
mod repository_data_source_hot_config;
mod repository_data_source_presets;
mod repository_data_source_status;
mod repository_enrollment;
mod repository_file_transfer_sources;
mod repository_file_transfers;
mod repository_gateway_sessions;
mod repository_history;
mod repository_ingest;
mod repository_inventory;
mod repository_job_lifecycle;
mod repository_job_outputs;
mod repository_jobs;
mod repository_key_lifecycle;
mod repository_migrations;
mod repository_network;
mod repository_network_observations;
mod repository_network_recommendations;
mod repository_operator_totp;
mod repository_restores;
mod repository_rollout_delegations;
mod repository_rollout_policies;
mod repository_rollouts;
mod repository_scheduled_jobs;
mod repository_schedules;
mod repository_telemetry_rollups;
mod repository_terminal_sessions;
mod repository_topology_graph;
mod routes;
mod routes_alerts;
mod routes_auth;
mod routes_backups;
mod routes_command_templates;
mod routes_discovery;
mod routes_enrollment;
mod routes_file_transfers;
mod routes_history;
mod routes_ingest;
mod routes_inventory;
mod routes_job_history;
mod routes_jobs;
mod routes_migrations;
mod routes_network;
mod routes_restores;
mod routes_rollout_policies;
mod routes_rollouts;
mod routes_schedules;
mod routes_terminal_sessions;
mod routes_update_releases;
mod routes_ws;
mod security;
mod state;
mod util;

use anyhow::{Context, Result};
use axum::http::StatusCode;
use clap::Parser;
use fleet_alerts::FleetAlertPolicy;
use gateway_client::{decode_server_signing_key, GatewayDispatchClient};
use model::{AuthContext, CreateJobRequest, OperatorView};
use object_store::{BackupObjectStore, S3BackupObjectStoreSettings};
use repository::Repository;
use repository_rollouts::DEFAULT_AGENT_UPDATE_HEARTBEAT_TIMEOUT_SECS;
use routes::build_router;
use state::{AppState, EnrollmentSettings, UpdateReleasePolicy};
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

pub(crate) use error::ApiError;
pub(crate) use routes_jobs::TargetDispatchOutcome;
pub(crate) use security::{
    generate_token, hash_operator_password, normalize_operator_scopes, token_hash,
    verify_operator_password, ACCESS_TOKEN_TTL_SECS, REFRESH_TOKEN_TTL_SECS,
};
pub(crate) use util::{output_stream_name, unix_now};

#[cfg(test)]
use axum::http::HeaderMap;
#[cfg(test)]
use ed25519_dalek::SigningKey;
#[cfg(test)]
use model::*;
#[cfg(test)]
use model_alert_notifications::*;
#[cfg(test)]
use model_alert_policies::*;
#[cfg(test)]
use model_alert_states::*;
#[cfg(test)]
use repository::MemoryState;
#[cfg(test)]
use repository_enrollment::EnrollmentClaimOutcome;
#[cfg(test)]
use repository_ingest::upsert_memory_agent;
#[cfg(test)]
use routes_schedules::validate_schedule_request;
#[cfg(test)]
use security::{constant_time_eq, role_allows, validate_operator_role};
#[cfg(test)]
use uuid::Uuid;
#[cfg(test)]
use vpsman_common::{encode_json, payload_hash, CommandOutput, OutputStream};
use vpsman_common::{AgentNoiseMode, AgentUpdateConfig, JobCommand, ServerEndpoint};

#[derive(Debug, Parser)]
#[command(name = "vpsman-api", about = "VPS control-plane API")]
struct Args {
    #[arg(long, env = "VPSMAN_API_BIND", default_value = "0.0.0.0:8080")]
    bind: SocketAddr,
    #[arg(long, env = "VPSMAN_POSTGRES_URL")]
    postgres_url: Option<String>,
    #[arg(long, env = "VPSMAN_MIGRATIONS_DIR", default_value = "migrations")]
    migrations_dir: PathBuf,
    #[arg(long, env = "VPSMAN_INTERNAL_TOKEN")]
    internal_token: Option<String>,
    #[arg(long, env = "VPSMAN_GATEWAY_CONTROL_URL")]
    gateway_control_url: Option<String>,
    #[arg(long, env = "VPSMAN_SERVER_SIGNING_KEY_HEX")]
    server_signing_key_hex: Option<String>,
    #[arg(
        long,
        env = "VPSMAN_DISCOVERY_TRUSTED_SERVER_PUBLIC_KEYS_HEX",
        value_delimiter = ','
    )]
    discovery_trusted_server_public_keys_hex: Vec<String>,
    #[arg(long, env = "VPSMAN_PUBLIC_GATEWAY_ENDPOINTS", value_delimiter = ',')]
    public_gateway_endpoints: Vec<String>,
    #[arg(long, env = "VPSMAN_DISCOVERY_URL")]
    discovery_url: Option<String>,
    #[arg(long, env = "VPSMAN_GATEWAY_SERVER_PUBLIC_KEY_HEX")]
    gateway_server_public_key_hex: Option<String>,
    #[arg(
        long,
        env = "VPSMAN_ENROLLMENT_TELEMETRY_LIGHT_SECS",
        default_value_t = 15
    )]
    enrollment_telemetry_light_secs: u64,
    #[arg(
        long,
        env = "VPSMAN_ENROLLMENT_TELEMETRY_FULL_SECS",
        default_value_t = 60
    )]
    enrollment_telemetry_full_secs: u64,
    #[arg(long, env = "VPSMAN_ENROLLMENT_DEFAULT_COUNTRY", default_value = "US")]
    enrollment_default_country: String,
    #[arg(
        long,
        env = "VPSMAN_ENROLLMENT_UNMANAGED_UPDATE_ENABLED",
        default_value_t = true
    )]
    enrollment_unmanaged_update_enabled: bool,
    #[arg(long, env = "VPSMAN_ENROLLMENT_UNMANAGED_UPDATE_VERSION_URL")]
    enrollment_unmanaged_update_version_url: Option<String>,
    #[arg(
        long,
        env = "VPSMAN_ENROLLMENT_UNMANAGED_UPDATE_INTERVAL_SECS",
        default_value_t = 86_400
    )]
    enrollment_unmanaged_update_interval_secs: u64,
    #[arg(
        long,
        env = "VPSMAN_ENROLLMENT_UNMANAGED_UPDATE_JITTER_SECS",
        default_value_t = 86_400
    )]
    enrollment_unmanaged_update_jitter_secs: u64,
    #[arg(
        long,
        env = "VPSMAN_ENROLLMENT_UNMANAGED_UPDATE_ACTIVATE",
        default_value_t = true
    )]
    enrollment_unmanaged_update_activate: bool,
    #[arg(
        long,
        env = "VPSMAN_ENROLLMENT_UNMANAGED_UPDATE_RESTART_AGENT",
        default_value_t = true
    )]
    enrollment_unmanaged_update_restart_agent: bool,
    #[arg(long, env = "VPSMAN_BACKUP_OBJECT_STORE_DIR")]
    backup_object_store_dir: Option<PathBuf>,
    #[arg(long, env = "VPSMAN_UPDATE_OBJECT_STORE_DIR")]
    update_object_store_dir: Option<PathBuf>,
    #[arg(long, env = "VPSMAN_UPDATE_OBJECT_ENDPOINT")]
    update_object_endpoint: Option<String>,
    #[arg(long, env = "VPSMAN_UPDATE_OBJECT_BUCKET")]
    update_object_bucket: Option<String>,
    #[arg(long, env = "VPSMAN_UPDATE_OBJECT_ACCESS_KEY")]
    update_object_access_key: Option<String>,
    #[arg(long, env = "VPSMAN_UPDATE_OBJECT_SECRET_KEY")]
    update_object_secret_key: Option<String>,
    #[arg(long, env = "VPSMAN_UPDATE_OBJECT_REGION", default_value = "us-east-1")]
    update_object_region: String,
    #[arg(
        long,
        env = "VPSMAN_UPDATE_OBJECT_CREATE_BUCKET",
        default_value_t = false
    )]
    update_object_create_bucket: bool,
    #[arg(long, env = "VPSMAN_UPDATE_ARTIFACT_PUBLIC_BASE_URL")]
    update_artifact_public_base_url: Option<String>,
    #[arg(
        long,
        env = "VPSMAN_AGENT_UPDATE_ALLOWED_CHANNELS",
        value_delimiter = ','
    )]
    agent_update_allowed_channels: Vec<String>,
    #[arg(
        long,
        env = "VPSMAN_AGENT_UPDATE_TRUSTED_SIGNING_KEYS_HEX",
        value_delimiter = ','
    )]
    agent_update_trusted_signing_keys_hex: Vec<String>,
    #[arg(long, env = "VPSMAN_OBJECT_ENDPOINT")]
    object_endpoint: Option<String>,
    #[arg(long, env = "VPSMAN_OBJECT_BUCKET")]
    object_bucket: Option<String>,
    #[arg(long, env = "VPSMAN_OBJECT_ACCESS_KEY")]
    object_access_key: Option<String>,
    #[arg(long, env = "VPSMAN_OBJECT_SECRET_KEY")]
    object_secret_key: Option<String>,
    #[arg(long, env = "VPSMAN_OBJECT_REGION", default_value = "us-east-1")]
    object_region: String,
    #[arg(long, env = "VPSMAN_OBJECT_CREATE_BUCKET", default_value_t = false)]
    object_create_bucket: bool,
    #[arg(
        long,
        env = "VPSMAN_JOB_OUTPUT_ARTIFACT_MIN_BYTES",
        default_value_t = 32768
    )]
    job_output_artifact_min_bytes: usize,
    #[arg(
        long,
        env = "VPSMAN_AGENT_UPDATE_HEARTBEAT_TIMEOUT_SECS",
        default_value_t = DEFAULT_AGENT_UPDATE_HEARTBEAT_TIMEOUT_SECS as u64
    )]
    agent_update_heartbeat_timeout_secs: u64,
    #[arg(
        long,
        env = "VPSMAN_AGENT_UPDATE_RECONCILE_INTERVAL_SECS",
        default_value_t = 30
    )]
    agent_update_reconcile_interval_secs: u64,
    #[arg(
        long,
        env = "VPSMAN_REQUIRE_REGISTERED_AGENT_UPDATES",
        default_value_t = false
    )]
    require_registered_agent_updates: bool,
    #[arg(
        long,
        env = "VPSMAN_ALERT_MEMORY_AVAILABLE_WARNING_RATIO",
        default_value_t = 0.20
    )]
    alert_memory_available_warning_ratio: f64,
    #[arg(
        long,
        env = "VPSMAN_ALERT_MEMORY_AVAILABLE_CRITICAL_RATIO",
        default_value_t = 0.10
    )]
    alert_memory_available_critical_ratio: f64,
    #[arg(
        long,
        env = "VPSMAN_ALERT_DISK_AVAILABLE_WARNING_RATIO",
        default_value_t = 0.20
    )]
    alert_disk_available_warning_ratio: f64,
    #[arg(
        long,
        env = "VPSMAN_ALERT_DISK_AVAILABLE_CRITICAL_RATIO",
        default_value_t = 0.10
    )]
    alert_disk_available_critical_ratio: f64,
    #[arg(long, env = "VPSMAN_ALERT_CPU_LOAD_WARNING", default_value_t = 2.0)]
    alert_cpu_load_warning: f64,
    #[arg(long, env = "VPSMAN_ALERT_CPU_LOAD_CRITICAL", default_value_t = 4.0)]
    alert_cpu_load_critical: f64,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "vpsman_api=info,tower_http=info".into()),
        )
        .init();

    let args = Args::parse();
    let repo = Repository::connect(args.postgres_url.as_deref(), &args.migrations_dir).await?;
    let (events, _) = broadcast::channel(256);
    let internal_token = required_internal_token(args.internal_token.as_deref())?;
    let gateway = GatewayDispatchClient::new(
        args.gateway_control_url.clone(),
        Some(internal_token.clone()),
    );
    let server_signing_key = args
        .server_signing_key_hex
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(decode_server_signing_key)
        .transpose()?
        .map(Arc::new);
    if server_signing_key.is_none() {
        warn!(
            "VPSMAN_SERVER_SIGNING_KEY_HEX is not configured; proof-gated job dispatch remains disabled"
        );
    }
    let server_ed25519_public_key_hex = server_signing_key
        .as_deref()
        .map(|key| hex::encode(key.verifying_key().to_bytes()));
    let discovery_trusted_server_ed25519_public_keys_hex =
        parse_public_key_ring(&args.discovery_trusted_server_public_keys_hex)?;
    let enrollment = EnrollmentSettings {
        tcp_endpoints: parse_public_gateway_endpoints(&args.public_gateway_endpoints)?,
        discovery_url: args.discovery_url.clone().filter(|value| !value.is_empty()),
        noise_mode: AgentNoiseMode::EnrolledIk,
        gateway_server_public_key_hex: args
            .gateway_server_public_key_hex
            .clone()
            .filter(|value| !value.is_empty()),
        server_ed25519_public_key_hex,
        discovery_trusted_server_ed25519_public_keys_hex,
        telemetry_light_secs: args.enrollment_telemetry_light_secs.max(5),
        telemetry_full_secs: args.enrollment_telemetry_full_secs.max(5),
        default_country_tag: default_country_tag(args.enrollment_default_country.as_str())?,
        update: AgentUpdateConfig {
            trusted_artifact_signing_key_hex: None,
            unmanaged_enabled: args.enrollment_unmanaged_update_enabled,
            unmanaged_version_url: args
                .enrollment_unmanaged_update_version_url
                .clone()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(vpsman_common::default_agent_unmanaged_update_version_url),
            unmanaged_interval_secs: args.enrollment_unmanaged_update_interval_secs,
            unmanaged_jitter_secs: args.enrollment_unmanaged_update_jitter_secs,
            unmanaged_activate: args.enrollment_unmanaged_update_activate,
            unmanaged_restart_agent: args.enrollment_unmanaged_update_restart_agent,
        },
    };
    let backup_object_store = build_backup_object_store(&args)?;
    if let Some(store) = &backup_object_store {
        info!(kind = store.kind(), "backup object store enabled");
    } else {
        warn!("backup object store is not configured; encrypted artifact upload is disabled");
    }
    let update_object_store = build_update_object_store(&args)?;
    let update_artifact_public_base_url =
        parse_public_update_artifact_base_url(args.update_artifact_public_base_url.as_deref())?;
    let update_release_policy = UpdateReleasePolicy::new(
        args.agent_update_allowed_channels.clone(),
        args.agent_update_trusted_signing_keys_hex.clone(),
    )?;
    let fleet_alert_policy = FleetAlertPolicy::new(
        args.alert_memory_available_warning_ratio,
        args.alert_memory_available_critical_ratio,
        args.alert_disk_available_warning_ratio,
        args.alert_disk_available_critical_ratio,
        args.alert_cpu_load_warning,
        args.alert_cpu_load_critical,
    )?;
    info!(
        allowed_channels = args.agent_update_allowed_channels.len(),
        trusted_signing_keys = args.agent_update_trusted_signing_keys_hex.len(),
        "agent update release policy configured"
    );
    if let Some(store) = &update_object_store {
        info!(
            kind = store.kind(),
            "agent update artifact object store enabled"
        );
    } else {
        warn!("agent update artifact object store is not configured; hosted update uploads are disabled");
    }
    let state = AppState {
        repo,
        events,
        internal_token: Some(internal_token),
        gateway,
        server_signing_key,
        enrollment,
        backup_object_store,
        update_object_store,
        update_artifact_public_base_url,
        update_release_policy,
        fleet_alert_policy,
        job_output_artifact_min_bytes: args.job_output_artifact_min_bytes,
        require_registered_agent_updates: args.require_registered_agent_updates,
    };
    spawn_agent_update_rollout_reconciler(
        state.clone(),
        args.agent_update_heartbeat_timeout_secs,
        args.agent_update_reconcile_interval_secs,
    );

    let listener = tokio::net::TcpListener::bind(args.bind)
        .await
        .with_context(|| format!("failed to bind API on {}", args.bind))?;
    info!(bind = %args.bind, "api listening");
    axum::serve(listener, build_router(state)).await?;
    Ok(())
}

fn required_internal_token(value: Option<&str>) -> Result<String> {
    let token = value
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .context("VPSMAN_INTERNAL_TOKEN is required")?;
    anyhow::ensure!(
        token.len() >= 32,
        "VPSMAN_INTERNAL_TOKEN must be at least 32 characters"
    );
    anyhow::ensure!(
        !matches!(
            token,
            "change-me"
                | "change-me-internal-token"
                | "replace-with-random-token-at-least-32-chars"
        ),
        "VPSMAN_INTERNAL_TOKEN must be changed from the deployment template placeholder"
    );
    Ok(token.to_string())
}

fn spawn_agent_update_rollout_reconciler(
    state: AppState,
    heartbeat_timeout_secs: u64,
    reconcile_interval_secs: u64,
) {
    let heartbeat_timeout_secs = heartbeat_timeout_secs.clamp(1, 86_400) as i32;
    let reconcile_interval_secs = reconcile_interval_secs.clamp(5, 3600);
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(reconcile_interval_secs));
        loop {
            ticker.tick().await;
            match state
                .repo
                .expire_agent_update_heartbeat_timeouts(heartbeat_timeout_secs)
                .await
            {
                Ok(expired) if expired > 0 => {
                    info!(expired, "agent update heartbeat timeouts reconciled");
                }
                Ok(_) => {
                    debug!("agent update heartbeat timeout reconciliation completed");
                }
                Err(error) => {
                    warn!(%error, "agent update heartbeat timeout reconciliation failed");
                }
            }
            match state.repo.expire_agent_update_delegated_proofs(500).await {
                Ok(expired) if expired > 0 => {
                    info!(expired, "delegated rollout proofs expired");
                }
                Ok(_) => {
                    debug!("delegated rollout proof expiry reconciliation completed");
                }
                Err(error) => {
                    warn!(%error, "delegated rollout proof expiry reconciliation failed");
                }
            }
            match dispatch_delegated_rollout_rollbacks(&state).await {
                Ok(dispatched) if dispatched > 0 => {
                    info!(dispatched, "delegated rollout rollback jobs dispatched");
                }
                Ok(_) => {
                    debug!("delegated rollout rollback reconciliation completed");
                }
                Err(error) => {
                    warn!(%error, "delegated rollout rollback reconciliation failed");
                }
            }
            match dispatch_delegated_rollout_activations(&state).await {
                Ok(dispatched) if dispatched > 0 => {
                    info!(dispatched, "delegated rollout activation jobs dispatched");
                }
                Ok(_) => {
                    debug!("delegated rollout activation reconciliation completed");
                }
                Err(error) => {
                    warn!(%error, "delegated rollout activation reconciliation failed");
                }
            }
        }
    });
}

async fn dispatch_delegated_rollout_rollbacks(state: &AppState) -> Result<usize> {
    let claims = state
        .repo
        .claim_ready_agent_update_rollback_delegations(25)
        .await?;
    let mut dispatched = 0_usize;
    for claim in claims {
        let operator = delegated_rollback_operator(claim.actor_id);
        let operation = JobCommand::AgentUpdateRollback {
            rollback_sha256_hex: claim.rollback_sha256_hex.clone(),
        };
        let request = CreateJobRequest {
            targets: Vec::new(),
            clients: claim.clients.clone(),
            pools: Vec::new(),
            tags: Vec::new(),
            tag_mode: None,
            destructive: false,
            confirmed: true,
            command: "agent_update_rollback".to_string(),
            argv: Vec::new(),
            operation: Some(operation),
            timeout_secs: Some(60),
            canary_count: None,
            force_unprivileged: claim.force_unprivileged,
            privileged: true,
            idempotency_key: Some(format!(
                "delegated:rollout-rollback:{}:{}",
                claim.rollout_id,
                claim.rollback_sha256_hex.as_deref().unwrap_or("default")
            )),
            reconnect_policy: Some(serde_json::json!({
                "duplicate_delivery": "ignore_completed",
                "resume_outputs": true,
                "cancel_on_disconnect": false,
            })),
            envelope: None,
            envelopes: claim.envelopes.clone(),
        };
        match routes_jobs::create_job_with_operator(state, &operator, request).await {
            Ok((StatusCode::ACCEPTED, response)) => {
                state
                    .repo
                    .mark_agent_update_rollback_delegations_dispatched(
                        claim.rollout_id,
                        &claim.delegation_ids,
                        response.0.job_id,
                    )
                    .await?;
                dispatched += 1;
            }
            Ok((status, response)) => {
                state
                    .repo
                    .mark_agent_update_rollback_delegations_failed(
                        claim.rollout_id,
                        &claim.delegation_ids,
                        &format!(
                            "delegated rollback dispatch returned {} with job {} status {}",
                            status, response.0.job_id, response.0.status
                        ),
                    )
                    .await?;
            }
            Err(error) => {
                let reason = format!("delegated rollback dispatch failed: {}", error.code);
                state
                    .repo
                    .mark_agent_update_rollback_delegations_failed(
                        claim.rollout_id,
                        &claim.delegation_ids,
                        &reason,
                    )
                    .await?;
            }
        }
    }
    Ok(dispatched)
}

async fn dispatch_delegated_rollout_activations(state: &AppState) -> Result<usize> {
    let claims = state
        .repo
        .claim_ready_agent_update_activation_delegations(25)
        .await?;
    let mut dispatched = 0_usize;
    for claim in claims {
        let operator = delegated_rollout_operator(claim.actor_id);
        let operation = JobCommand::AgentUpdateActivate {
            staged_sha256_hex: claim.staged_sha256_hex.clone(),
            restart_agent: claim.restart_agent,
        };
        let request = CreateJobRequest {
            targets: Vec::new(),
            clients: claim.clients.clone(),
            pools: Vec::new(),
            tags: Vec::new(),
            tag_mode: None,
            destructive: false,
            confirmed: true,
            command: "agent_update_activate".to_string(),
            argv: Vec::new(),
            operation: Some(operation),
            timeout_secs: Some(60),
            canary_count: None,
            force_unprivileged: claim.force_unprivileged,
            privileged: true,
            idempotency_key: Some(format!(
                "delegated:rollout-activate:{}:{}",
                claim.rollout_id, claim.staged_sha256_hex
            )),
            reconnect_policy: Some(serde_json::json!({
                "duplicate_delivery": "ignore_completed",
                "resume_outputs": true,
                "cancel_on_disconnect": false,
            })),
            envelope: None,
            envelopes: claim.envelopes.clone(),
        };
        match routes_jobs::create_job_with_operator(state, &operator, request).await {
            Ok((StatusCode::ACCEPTED, response)) => {
                state
                    .repo
                    .mark_agent_update_activation_delegations_dispatched(
                        claim.rollout_id,
                        &claim.delegation_ids,
                        response.0.job_id,
                    )
                    .await?;
                dispatched += 1;
            }
            Ok((status, response)) => {
                state
                    .repo
                    .mark_agent_update_activation_delegations_failed(
                        claim.rollout_id,
                        &claim.delegation_ids,
                        &format!(
                            "delegated activation dispatch returned {} with job {} status {}",
                            status, response.0.job_id, response.0.status
                        ),
                    )
                    .await?;
            }
            Err(error) => {
                let reason = format!("delegated activation dispatch failed: {}", error.code);
                state
                    .repo
                    .mark_agent_update_activation_delegations_failed(
                        claim.rollout_id,
                        &claim.delegation_ids,
                        &reason,
                    )
                    .await?;
            }
        }
    }
    Ok(dispatched)
}

fn delegated_rollback_operator(actor_id: Option<uuid::Uuid>) -> AuthContext {
    delegated_rollout_operator(actor_id)
}

fn delegated_rollout_operator(actor_id: Option<uuid::Uuid>) -> AuthContext {
    AuthContext {
        operator: OperatorView {
            id: actor_id.unwrap_or_else(uuid::Uuid::nil),
            username: "rollout-automation".to_string(),
            role: "operator".to_string(),
            scopes: vec!["jobs:write".to_string()],
            totp_enabled: false,
        },
        session_id: uuid::Uuid::nil(),
    }
}

fn parse_public_gateway_endpoints(values: &[String]) -> Result<Vec<ServerEndpoint>> {
    if values.is_empty() {
        return Ok(EnrollmentSettings::default().tcp_endpoints);
    }
    values
        .iter()
        .map(|value| {
            let parts = value.split('=').collect::<Vec<_>>();
            anyhow::ensure!(
                parts.len() == 3,
                "gateway endpoint must use label=tcp_addr=priority"
            );
            let priority = parts[2]
                .parse::<u16>()
                .with_context(|| format!("invalid endpoint priority in {value}"))?;
            anyhow::ensure!(!parts[0].trim().is_empty(), "endpoint label is empty");
            anyhow::ensure!(!parts[1].trim().is_empty(), "endpoint tcp_addr is empty");
            Ok(ServerEndpoint {
                label: parts[0].trim().to_string(),
                tcp_addr: parts[1].trim().to_string(),
                priority,
            })
        })
        .collect()
}

fn parse_public_key_ring(values: &[String]) -> Result<Vec<String>> {
    let mut keys = values
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>();
    keys.sort();
    keys.dedup();
    anyhow::ensure!(
        keys.len() <= 8,
        "at most 8 discovery trusted server public keys can be configured"
    );
    for key in &keys {
        let decoded =
            hex::decode(key).context("invalid discovery trusted server public key hex")?;
        anyhow::ensure!(
            decoded.len() == 32,
            "discovery trusted server public key must be 32 bytes"
        );
    }
    Ok(keys)
}

fn build_backup_object_store(args: &Args) -> Result<Option<BackupObjectStore>> {
    if let Some(store) = args
        .backup_object_store_dir
        .clone()
        .filter(|path| !path.as_os_str().is_empty())
        .map(BackupObjectStore::filesystem)
        .transpose()?
    {
        return Ok(Some(store));
    }

    if let Some(store) = build_s3_object_store(
        &args.object_endpoint,
        &args.object_bucket,
        &args.object_access_key,
        &args.object_secret_key,
        &args.object_region,
        args.object_create_bucket,
        "S3 object storage requires VPSMAN_OBJECT_ENDPOINT, VPSMAN_OBJECT_BUCKET, VPSMAN_OBJECT_ACCESS_KEY, and VPSMAN_OBJECT_SECRET_KEY",
    )? {
        return Ok(Some(store));
    }

    Ok(None)
}

fn build_update_object_store(args: &Args) -> Result<Option<BackupObjectStore>> {
    if let Some(store) = args
        .update_object_store_dir
        .clone()
        .filter(|path| !path.as_os_str().is_empty())
        .map(BackupObjectStore::filesystem)
        .transpose()?
    {
        return Ok(Some(store));
    }

    if let Some(store) = build_s3_object_store(
        &args.update_object_endpoint,
        &args.update_object_bucket,
        &args.update_object_access_key,
        &args.update_object_secret_key,
        &args.update_object_region,
        args.update_object_create_bucket,
        "S3 update object storage requires VPSMAN_UPDATE_OBJECT_ENDPOINT, VPSMAN_UPDATE_OBJECT_BUCKET, VPSMAN_UPDATE_OBJECT_ACCESS_KEY, and VPSMAN_UPDATE_OBJECT_SECRET_KEY",
    )? {
        return Ok(Some(store));
    }

    Ok(None)
}

fn parse_public_update_artifact_base_url(value: Option<&str>) -> Result<Option<String>> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    anyhow::ensure!(
        value.starts_with("https://"),
        "VPSMAN_UPDATE_ARTIFACT_PUBLIC_BASE_URL must start with https://"
    );
    anyhow::ensure!(
        !value.as_bytes().contains(&0),
        "VPSMAN_UPDATE_ARTIFACT_PUBLIC_BASE_URL contains a NUL byte"
    );
    Ok(Some(value.trim_end_matches('/').to_string()))
}

fn default_country_tag(value: &str) -> Result<Option<String>> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    anyhow::ensure!(
        value.len() <= 32
            && value
                .chars()
                .all(|character| character.is_ascii_alphanumeric()
                    || character == '-'
                    || character == '_'),
        "VPSMAN_ENROLLMENT_DEFAULT_COUNTRY must be a short country code/tag fragment"
    );
    Ok(Some(format!("country:{}", value.to_ascii_uppercase())))
}

fn build_s3_object_store(
    endpoint: &Option<String>,
    bucket: &Option<String>,
    access_key: &Option<String>,
    secret_key: &Option<String>,
    region: &str,
    create_bucket: bool,
    incomplete_config_message: &'static str,
) -> Result<Option<BackupObjectStore>> {
    let s3_fields = [
        endpoint.as_deref(),
        bucket.as_deref(),
        access_key.as_deref(),
        secret_key.as_deref(),
    ];
    let s3_field_count = s3_fields
        .iter()
        .filter(|value| value.is_some_and(|value| !value.trim().is_empty()))
        .count();
    if s3_field_count == 0 {
        return Ok(None);
    }
    anyhow::ensure!(s3_field_count == s3_fields.len(), incomplete_config_message);
    Ok(Some(BackupObjectStore::s3(S3BackupObjectStoreSettings {
        endpoint: endpoint.clone().unwrap_or_default(),
        bucket: bucket.clone().unwrap_or_default(),
        access_key: access_key.clone().unwrap_or_default(),
        secret_key: secret_key.clone().unwrap_or_default(),
        region: region.to_string(),
        create_bucket,
    })?))
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_alerts;
#[cfg(test)]
mod tests_auth;
#[cfg(test)]
mod tests_auth_rotation;
#[cfg(test)]
mod tests_backups;
#[cfg(test)]
mod tests_config;
#[cfg(test)]
mod tests_data_sources;
#[cfg(test)]
mod tests_files;
#[cfg(test)]
mod tests_history;
#[cfg(test)]
mod tests_identity;
#[cfg(test)]
mod tests_migrations;
#[cfg(test)]
mod tests_network;
#[cfg(test)]
mod tests_network_adapter_promotion;
#[cfg(test)]
mod tests_network_observations;
#[cfg(test)]
mod tests_network_ospf_cost_update;
#[cfg(test)]
mod tests_network_ospf_updates;
#[cfg(test)]
mod tests_network_telemetry;
#[cfg(test)]
mod tests_network_telemetry_promotion;
#[cfg(test)]
mod tests_object_store;
#[cfg(test)]
mod tests_process;
#[cfg(test)]
mod tests_restores;
#[cfg(test)]
mod tests_rollout_activation_failure;
#[cfg(test)]
mod tests_rollouts;
#[cfg(test)]
mod tests_schedules;
#[cfg(test)]
mod tests_terminal;
#[cfg(test)]
mod tests_update_releases;
