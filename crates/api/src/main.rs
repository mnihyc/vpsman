use std::{net::SocketAddr, path::PathBuf};

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
mod job_dispatcher;
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
mod model_server_jobs;
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
mod repository_server_dashboard;
mod repository_server_jobs;
mod repository_suite_config;
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
mod routes_server_jobs;
mod routes_suite_config;
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
use gateway_client::GatewayDispatchClient;
use object_store::{BackupObjectStore, S3BackupObjectStoreSettings};
use repository::Repository;
use routes::build_router;
use state::{AppState, UpdateReleasePolicy};
use tokio::sync::broadcast;
use tracing::info;
use vpsman_common::{read_secret_file_ref, SuiteConfig};

pub(crate) use error::ApiError;
pub(crate) use routes_jobs::TargetDispatchOutcome;
pub(crate) use security::{
    generate_token, hash_operator_password, normalize_operator_scopes, token_hash,
    verify_operator_password, ACCESS_TOKEN_TTL_SECS, REFRESH_TOKEN_TTL_SECS,
};
pub(crate) use util::{output_stream_name, unix_now};

#[cfg(test)]
pub(crate) async fn test_auth_context_and_headers(state: &AppState) -> (AuthContext, HeaderMap) {
    let operator = OperatorRecord {
        id: Uuid::new_v4(),
        username: format!("test-admin-{}", Uuid::new_v4()),
        password_hash: "test-only-session-issued-directly".to_string(),
        role: "admin".to_string(),
        scopes: vec!["*".to_string()],
        preferences: OperatorPreferences::default(),
        totp_enabled: false,
        totp_secret_ciphertext_hex: None,
        totp_secret_nonce_hex: None,
        totp_secret_salt_hex: None,
    };
    if let Repository::Memory(memory) = &state.repo {
        memory.operators.write().await.push(operator.clone());
    } else {
        panic!("test_auth_context_and_headers currently supports the memory repository");
    }
    let auth = state
        .repo
        .issue_session(operator.view())
        .await
        .expect("test operator session");
    let context = state
        .repo
        .authenticate_access_token(&auth.access_token)
        .await
        .expect("test access token auth")
        .expect("test access token context");
    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        format!("Bearer {}", auth.access_token)
            .parse()
            .expect("test bearer header"),
    );
    (context, headers)
}

#[cfg(test)]
pub(crate) async fn test_auth_headers(state: &AppState) -> HeaderMap {
    test_auth_context_and_headers(state).await.1
}

#[cfg(test)]
use axum::http::{header::AUTHORIZATION, HeaderMap};
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
use routes_schedules::validate_schedule_request;
#[cfg(test)]
use security::{constant_time_eq, role_allows, validate_operator_role};
use uuid::Uuid;
#[cfg(test)]
use vpsman_common::{encode_json, payload_hash, CommandOutput, OutputStream};

#[derive(Debug, Parser)]
#[command(name = "vpsman-api", about = "VPS control-plane API")]
struct Args {
    #[arg(
        long,
        env = "VPSMAN_SUITE_CONFIG",
        default_value = "config/vpsman.toml"
    )]
    suite_config: PathBuf,
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

impl Args {
    fn apply_suite_config(&mut self, config: &SuiteConfig) -> std::result::Result<(), String> {
        if env_absent("VPSMAN_API_BIND") {
            if let Some(bind) = config.api.bind.as_deref() {
                self.bind = bind
                    .parse()
                    .map_err(|error| format!("api.bind_invalid:{error}"))?;
            }
        }
        apply_opt_string(
            &mut self.postgres_url,
            "VPSMAN_POSTGRES_URL",
            config.database.postgres_url.as_deref(),
        );
        apply_path_default(
            &mut self.migrations_dir,
            "VPSMAN_MIGRATIONS_DIR",
            config.database.migrations_dir.as_deref(),
        );
        apply_opt_string(
            &mut self.gateway_control_url,
            "VPSMAN_GATEWAY_CONTROL_URL",
            config.api.gateway_control_url.as_deref(),
        );
        apply_opt_path(
            &mut self.backup_object_store_dir,
            "VPSMAN_BACKUP_OBJECT_STORE_DIR",
            config
                .storage
                .backup_object_store_dir
                .as_deref()
                .or(config.storage.object_store_dir.as_deref()),
        );
        apply_opt_path(
            &mut self.update_object_store_dir,
            "VPSMAN_UPDATE_OBJECT_STORE_DIR",
            config
                .storage
                .update_object_store_dir
                .as_deref()
                .or(config.storage.object_store_dir.as_deref()),
        );
        apply_opt_string(
            &mut self.object_endpoint,
            "VPSMAN_OBJECT_ENDPOINT",
            config.storage.object_endpoint.as_deref(),
        );
        apply_opt_string(
            &mut self.object_bucket,
            "VPSMAN_OBJECT_BUCKET",
            config.storage.object_bucket.as_deref(),
        );
        apply_opt_string(
            &mut self.update_object_endpoint,
            "VPSMAN_UPDATE_OBJECT_ENDPOINT",
            config.storage.update_object_endpoint.as_deref(),
        );
        apply_opt_string(
            &mut self.update_object_bucket,
            "VPSMAN_UPDATE_OBJECT_BUCKET",
            config.storage.update_object_bucket.as_deref(),
        );
        apply_string_default(
            &mut self.object_region,
            "VPSMAN_OBJECT_REGION",
            config.storage.object_region.as_deref(),
        );
        apply_string_default(
            &mut self.update_object_region,
            "VPSMAN_UPDATE_OBJECT_REGION",
            config.storage.update_object_region.as_deref(),
        );
        apply_bool_default(
            &mut self.object_create_bucket,
            "VPSMAN_OBJECT_CREATE_BUCKET",
            config.storage.object_create_bucket,
        );
        apply_bool_default(
            &mut self.update_object_create_bucket,
            "VPSMAN_UPDATE_OBJECT_CREATE_BUCKET",
            config.storage.update_object_create_bucket,
        );
        apply_opt_string(
            &mut self.update_artifact_public_base_url,
            "VPSMAN_UPDATE_ARTIFACT_PUBLIC_BASE_URL",
            config.api.update_artifact_public_base_url.as_deref(),
        );
        if env_absent("VPSMAN_JOB_OUTPUT_ARTIFACT_MIN_BYTES") {
            if let Some(value) = config.api.job_output_artifact_min_bytes {
                self.job_output_artifact_min_bytes = value;
            }
        }
        if env_absent("VPSMAN_REQUIRE_REGISTERED_AGENT_UPDATES") {
            if let Some(value) = config.api.require_registered_agent_updates {
                self.require_registered_agent_updates = value;
            }
        }
        if env_absent("VPSMAN_ALERT_MEMORY_AVAILABLE_WARNING_RATIO") {
            if let Some(value) = config.api.alert_memory_available_warning_ratio {
                self.alert_memory_available_warning_ratio = value;
            }
        }
        if env_absent("VPSMAN_ALERT_MEMORY_AVAILABLE_CRITICAL_RATIO") {
            if let Some(value) = config.api.alert_memory_available_critical_ratio {
                self.alert_memory_available_critical_ratio = value;
            }
        }
        if env_absent("VPSMAN_ALERT_DISK_AVAILABLE_WARNING_RATIO") {
            if let Some(value) = config.api.alert_disk_available_warning_ratio {
                self.alert_disk_available_warning_ratio = value;
            }
        }
        if env_absent("VPSMAN_ALERT_DISK_AVAILABLE_CRITICAL_RATIO") {
            if let Some(value) = config.api.alert_disk_available_critical_ratio {
                self.alert_disk_available_critical_ratio = value;
            }
        }
        if env_absent("VPSMAN_ALERT_CPU_LOAD_WARNING") {
            if let Some(value) = config.api.alert_cpu_load_warning {
                self.alert_cpu_load_warning = value;
            }
        }
        if env_absent("VPSMAN_ALERT_CPU_LOAD_CRITICAL") {
            if let Some(value) = config.api.alert_cpu_load_critical {
                self.alert_cpu_load_critical = value;
            }
        }
        if env_absent("VPSMAN_API_DB_MAX_CONNECTIONS") {
            if let Some(value) = config.capacity.api_db_pool {
                std::env::set_var("VPSMAN_API_DB_MAX_CONNECTIONS", value.to_string());
            }
        }
        if self.internal_token.is_none() && env_absent("VPSMAN_INTERNAL_TOKEN") {
            self.internal_token =
                read_secret_file_ref(config.secrets.internal_token_file.as_deref())?;
        }
        if self.object_access_key.is_none() && env_absent("VPSMAN_OBJECT_ACCESS_KEY") {
            self.object_access_key =
                read_secret_file_ref(config.secrets.object_access_key_file.as_deref())?;
        }
        if self.object_secret_key.is_none() && env_absent("VPSMAN_OBJECT_SECRET_KEY") {
            self.object_secret_key =
                read_secret_file_ref(config.secrets.object_secret_key_file.as_deref())?;
        }
        if self.update_object_access_key.is_none() && env_absent("VPSMAN_UPDATE_OBJECT_ACCESS_KEY")
        {
            self.update_object_access_key =
                read_secret_file_ref(config.secrets.update_object_access_key_file.as_deref())?;
        }
        if self.update_object_secret_key.is_none() && env_absent("VPSMAN_UPDATE_OBJECT_SECRET_KEY")
        {
            self.update_object_secret_key =
                read_secret_file_ref(config.secrets.update_object_secret_key_file.as_deref())?;
        }
        Ok(())
    }
}

fn env_absent(name: &str) -> bool {
    std::env::var_os(name).is_none()
}

fn apply_opt_string(target: &mut Option<String>, env_name: &str, value: Option<&str>) {
    if target.is_none() && env_absent(env_name) {
        if let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) {
            *target = Some(value.to_string());
        }
    }
}

fn apply_opt_path(target: &mut Option<PathBuf>, env_name: &str, value: Option<&str>) {
    if target.is_none() && env_absent(env_name) {
        if let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) {
            *target = Some(PathBuf::from(value));
        }
    }
}

fn apply_path_default(target: &mut PathBuf, env_name: &str, value: Option<&str>) {
    if env_absent(env_name) {
        if let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) {
            *target = PathBuf::from(value);
        }
    }
}

fn apply_string_default(target: &mut String, env_name: &str, value: Option<&str>) {
    if env_absent(env_name) {
        if let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) {
            *target = value.to_string();
        }
    }
}

fn apply_bool_default(target: &mut bool, env_name: &str, value: Option<bool>) {
    if env_absent(env_name) {
        if let Some(value) = value {
            *target = value;
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "vpsman_api=info,tower_http=info".into()),
        )
        .init();

    let mut args = Args::parse();
    let suite_config =
        SuiteConfig::load_optional(&args.suite_config).map_err(anyhow::Error::msg)?;
    args.apply_suite_config(&suite_config)
        .map_err(anyhow::Error::msg)?;
    info!(
        version = env!("CARGO_PKG_VERSION"),
        server_build_number = build_info::server_build_number(),
        "api build metadata"
    );
    reject_api_privilege_verifier_env()?;
    let repo = Repository::connect(args.postgres_url.as_deref(), &args.migrations_dir).await?;
    let (events, _) = broadcast::channel(256);
    let internal_token = required_internal_token(args.internal_token.as_deref())?;
    let gateway = GatewayDispatchClient::new(
        args.gateway_control_url.clone(),
        Some(internal_token.clone()),
    );
    let backup_configured_object_store = build_backup_object_store(&args)?;
    let update_configured_object_store = build_update_object_store(&args)?;
    let artifact_object_store = backup_configured_object_store
        .or(update_configured_object_store)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "artifact object storage must be configured with VPSMAN_BACKUP_OBJECT_STORE_DIR or VPSMAN_OBJECT_* S3 settings"
            )
        })?;
    info!(
        kind = artifact_object_store.kind(),
        "artifact object store enabled for backups, transfers, job outputs, and hosted updates"
    );
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
    let state = AppState {
        repo,
        events,
        internal_token: Some(internal_token),
        gateway,
        backup_object_store: Some(artifact_object_store.clone()),
        update_object_store: Some(artifact_object_store),
        update_artifact_public_base_url,
        update_release_policy,
        fleet_alert_policy,
        job_output_artifact_min_bytes: args.job_output_artifact_min_bytes,
        require_registered_agent_updates: args.require_registered_agent_updates,
        suite_config_path: args.suite_config.clone(),
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
                },
            }),
            actor_id: None,
        })
        .await?;
    job_dispatcher::spawn_job_dispatcher(state.clone());
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
