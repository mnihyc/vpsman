use std::{net::SocketAddr, path::PathBuf, sync::Arc};

mod agent_update_artifact_ingest;
mod auth_model;
mod auth_totp;
mod backup_artifact_crypto;
mod backup_auto_artifacts;
mod backup_handoff;
mod backup_upload_sessions;
mod build_info;
mod data_source_builtin_presets;
mod error;
mod fleet_alert_notifications;
mod fleet_alerts;
mod gateway_client;
mod job_files;
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
mod model_dashboard;
mod model_data_sources;
mod model_file_transfer;
mod model_history;
mod model_terminal;
mod model_topology;
mod model_webhook_rules;
mod object_store;
mod privilege;
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
mod repository_file_transfer_sources;
mod repository_file_transfers;
mod repository_gateway_sessions;
mod repository_history;
mod repository_hot_config_rule_templates;
mod repository_ingest;
mod repository_inventory;
mod repository_job_outputs;
mod repository_jobs;
mod repository_key_lifecycle;
mod repository_migrations;
mod repository_network;
mod repository_network_observations;
mod repository_network_recommendations;
mod repository_operator_totp;
mod repository_restores;
mod repository_schedules;
mod repository_telemetry_rollups;
mod repository_terminal_sessions;
mod repository_topology_graph;
mod repository_webhook_rules;
mod routes;
mod routes_alerts;
mod routes_auth;
mod routes_backups;
mod routes_command_templates;
mod routes_dashboard;
mod routes_file_transfers;
mod routes_history;
mod routes_ingest;
mod routes_inventory;
mod routes_job_history;
mod routes_jobs;
mod routes_key_lifecycle;
mod routes_migrations;
mod routes_network;
mod routes_restores;
mod routes_schedules;
mod routes_terminal_sessions;
mod routes_update_releases;
mod routes_webhook_rules;
mod routes_ws;
mod security;
mod selector_expression;
mod state;
mod util;
mod webhook_rules;

use anyhow::{Context, Result};
use clap::Parser;
use fleet_alerts::FleetAlertPolicy;
use gateway_client::{decode_server_signing_key, GatewayDispatchClient};
use object_store::{BackupObjectStore, S3BackupObjectStoreSettings};
use repository::Repository;
use routes::build_router;
use state::{AppState, UpdateReleasePolicy};
use tokio::sync::broadcast;
use tracing::{info, warn};

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
use repository_ingest::upsert_memory_agent;
#[cfg(test)]
use repository_key_lifecycle::KeyLifecycleTrustReport;
#[cfg(test)]
use routes_schedules::validate_schedule_request;
#[cfg(test)]
use security::{constant_time_eq, role_allows, validate_operator_role};
use uuid::Uuid;
#[cfg(test)]
use vpsman_common::{encode_json, payload_hash, CommandOutput, OutputStream};

#[derive(Debug, Parser)]
#[command(name = "vpsman-api", about = "VPS control-plane API")]
struct Args {
    #[arg(long, env = "VPSMAN_API_BIND", default_value = "0.0.0.0:8080")]
    bind: SocketAddr,
    #[arg(long, env = "VPSMAN_POSTGRES_URL")]
    postgres_url: Option<String>,
    #[arg(long, env = "VPSMAN_DEBUG_INTERNAL_TEST_MODE", default_value_t = false)]
    debug_internal_test_mode: bool,
    #[arg(long, env = "VPSMAN_MIGRATIONS_DIR", default_value = "migrations")]
    migrations_dir: PathBuf,
    #[arg(long, env = "VPSMAN_INTERNAL_TOKEN")]
    internal_token: Option<String>,
    #[arg(long, env = "VPSMAN_GATEWAY_CONTROL_URL")]
    gateway_control_url: Option<String>,
    #[arg(long, env = "VPSMAN_SERVER_SIGNING_KEY_HEX")]
    server_signing_key_hex: Option<String>,
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
    info!(
        version = env!("CARGO_PKG_VERSION"),
        server_build_number = build_info::server_build_number(),
        "api build metadata"
    );
    reject_api_privilege_verifier_env()?;
    let repo = Repository::connect(
        args.postgres_url.as_deref(),
        &args.migrations_dir,
        args.debug_internal_test_mode,
    )
    .await?;
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
        if !args.debug_internal_test_mode {
            anyhow::bail!(
                "VPSMAN_SERVER_SIGNING_KEY_HEX is required. Missing signing keys are allowed only with VPSMAN_DEBUG_INTERNAL_TEST_MODE=true for dangerous internal tests."
            );
        }
        warn!(
            "DANGEROUS INTERNAL TEST MODE: VPSMAN_SERVER_SIGNING_KEY_HEX is not configured; privilege-gated job dispatch remains disabled"
        );
    }
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
        backup_object_store,
        update_object_store,
        update_artifact_public_base_url,
        update_release_policy,
        fleet_alert_policy,
        job_output_artifact_min_bytes: args.job_output_artifact_min_bytes,
        require_registered_agent_updates: args.require_registered_agent_updates,
    };
    state
        .repo
        .record_webhook_event(crate::model_webhook_rules::WebhookEventCandidate {
            kind: "server.on_start".to_string(),
            event_id: format!("server.on_start:{}:{}", unix_now(), Uuid::new_v4()),
            event_predicates: vec!["server.on_start".to_string()],
            subject_client_ids: Vec::new(),
            payload: serde_json::json!({
                "event": {
                    "kind": "server.on_start",
                },
                "server": {
                    "version": env!("CARGO_PKG_VERSION"),
                    "server_build_number": build_info::server_build_number(),
                    "bind": args.bind.to_string(),
                    "debug_internal_test_mode": args.debug_internal_test_mode,
                },
            }),
            actor_id: None,
        })
        .await?;
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

fn reject_api_privilege_verifier_env() -> Result<()> {
    if let Some(name) = forbidden_api_privilege_env_var(|name| std::env::var_os(name).is_some()) {
        anyhow::bail!("{name} must not be present in the API environment");
    }
    Ok(())
}

fn forbidden_api_privilege_env_var(mut present: impl FnMut(&str) -> bool) -> Option<&'static str> {
    const FORBIDDEN_ENV: &[&str] = &["VPSMAN_PRIVILEGE_VERIFIER_KEY_HEX"];
    FORBIDDEN_ENV.iter().copied().find(|name| present(name))
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
fn test_selector_expression_for_clients<I, S>(clients: I) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    clients
        .into_iter()
        .map(|client| format!("id:{}", client.as_ref()))
        .collect::<Vec<_>>()
        .join(" || ")
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_alerts;
#[cfg(test)]
mod tests_auth;
#[cfg(test)]
#[cfg(test)]
mod tests_backups;
#[cfg(test)]
mod tests_config;
#[cfg(test)]
mod tests_dashboard;
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
mod tests_schedules;
#[cfg(test)]
mod tests_terminal;
#[cfg(test)]
mod tests_update_releases;
