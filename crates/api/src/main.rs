use std::{net::SocketAddr, path::PathBuf};

mod auth_model;
mod auth_totp;
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
pub(crate) mod object_store {
    pub(crate) use vpsman_object_store::*;
}
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
mod repository_server_jobs;
mod repository_suite_config;
mod repository_system_dashboard;
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
mod routes_system;
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
use gateway_client::{GatewayClientTimeouts, GatewayDispatchClient};
use object_store::{BackupObjectStore, S3BackupObjectStoreSettings};
use repository::Repository;
use routes::build_router;
use state::{AppState, UpdateReleasePolicy, DEFAULT_ARTIFACT_MAX_BYTES};
use tokio::{sync::broadcast, time};
use tracing::info;
use vpsman_common::{read_secret_file_ref, SuiteConfig};

const DEFAULT_BACKUP_OBJECT_STORE_DIR: &str = "deploy/runtime/data/objects/backups";

pub(crate) use error::ApiError;
pub(crate) use routes_jobs::TargetDispatchOutcome;
pub(crate) use security::{
    generate_token, hash_operator_password, normalize_operator_scopes, token_hash,
    verify_operator_password, ACCESS_TOKEN_TTL_SECS, DEFAULT_REFRESH_TOKEN_TTL_SECS,
    MAX_REFRESH_TOKEN_TTL_SECS, MIN_REFRESH_TOKEN_TTL_SECS,
};
pub(crate) use util::{output_stream_name, unix_now};

#[cfg(test)]
pub(crate) async fn test_auth_context_and_headers(state: &AppState) -> (AuthContext, HeaderMap) {
    let operator = OperatorRecord {
        id: Uuid::new_v4(),
        username: format!("test-admin-{}", Uuid::new_v4()),
        password_hash: "test-only-session-issued-directly".to_string(),
        status: "active".to_string(),
        role: "admin".to_string(),
        scopes: vec!["*".to_string()],
        preferences: OperatorPreferences::default(),
        totp_enabled: false,
        totp_secret_ciphertext_hex: None,
        totp_secret_nonce_hex: None,
        totp_secret_salt_hex: None,
        session_refresh_ttl_secs: DEFAULT_REFRESH_TOKEN_TTL_SECS,
        created_at: unix_now().to_string(),
        disabled_at: None,
        deleted_at: None,
    };
    if let Repository::Memory(memory) = &state.repo {
        memory.operators.write().await.push(operator.clone());
    } else {
        panic!("test_auth_context_and_headers currently supports the unit-test repository fixture");
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
use vpsman_common::{encode_json, payload_hash, OutputStream};

#[derive(Debug, Parser)]
#[command(name = "vpsman-api", about = "VPS control-plane API")]
struct Args {
    #[arg(
        long,
        env = "VPSMAN_SUITE_CONFIG",
        default_value = "config/vpsman.toml"
    )]
    suite_config: PathBuf,
    #[arg(long, env = "VPSMAN_API_BIND", default_value = "127.0.0.1:8080")]
    bind: SocketAddr,
    #[arg(long, env = "VPSMAN_POSTGRES_URL")]
    postgres_url: Option<String>,
    #[arg(long, env = "VPSMAN_MIGRATIONS_DIR", default_value = "migrations")]
    migrations_dir: PathBuf,
    #[arg(long, env = "VPSMAN_INTERNAL_TOKEN")]
    internal_token: Option<String>,
    #[arg(long, env = "VPSMAN_GATEWAY_CONTROL_URL")]
    gateway_control_url: Option<String>,
    #[arg(long, env = "VPSMAN_INTERNAL_HTTP_CONNECT_SECS", default_value_t = 10)]
    internal_http_connect_secs: u64,
    #[arg(long, env = "VPSMAN_INTERNAL_HTTP_WRITE_SECS", default_value_t = 10)]
    internal_http_write_secs: u64,
    #[arg(long, env = "VPSMAN_INTERNAL_HTTP_READ_SECS", default_value_t = 15)]
    internal_http_read_secs: u64,
    #[arg(long, env = "VPSMAN_DISPATCH_ACK_SECS", default_value_t = 30)]
    dispatch_ack_secs: u64,
    #[arg(long, env = "VPSMAN_EVENT_POST_SECS", default_value_t = 15)]
    event_post_secs: u64,
    #[arg(long, env = "VPSMAN_CONTROL_DEADLINE_GRACE_SECS", default_value_t = 30)]
    control_deadline_grace_secs: u64,
    #[arg(long, env = "VPSMAN_DISPATCHER_BATCH", default_value_t = 128)]
    dispatcher_batch: i64,
    #[arg(long, env = "VPSMAN_DISPATCHER_IN_FLIGHT", default_value_t = 64)]
    dispatcher_in_flight: usize,
    #[arg(long, env = "VPSMAN_BACKUP_OBJECT_STORE_DIR")]
    backup_object_store_dir: Option<PathBuf>,
    #[arg(
        long,
        env = "VPSMAN_AGENT_UPDATE_ALLOWED_CHANNELS",
        value_delimiter = ','
    )]
    agent_update_allowed_channels: Vec<String>,
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
        env = "VPSMAN_ARTIFACT_MAX_BYTES",
        default_value_t = DEFAULT_ARTIFACT_MAX_BYTES
    )]
    artifact_max_bytes: usize,
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
        if self.gateway_control_url.is_none() && env_absent("VPSMAN_GATEWAY_CONTROL_URL") {
            self.gateway_control_url = Some("unix:./runtime/gateway-control.sock".to_string());
        }
        apply_u64_default(
            &mut self.internal_http_connect_secs,
            "VPSMAN_INTERNAL_HTTP_CONNECT_SECS",
            config.timeout.internal_http_connect_secs,
        );
        apply_u64_default(
            &mut self.internal_http_write_secs,
            "VPSMAN_INTERNAL_HTTP_WRITE_SECS",
            config.timeout.internal_http_write_secs,
        );
        apply_u64_default(
            &mut self.internal_http_read_secs,
            "VPSMAN_INTERNAL_HTTP_READ_SECS",
            config.timeout.internal_http_read_secs,
        );
        apply_u64_default(
            &mut self.dispatch_ack_secs,
            "VPSMAN_DISPATCH_ACK_SECS",
            config.timeout.dispatch_ack_secs,
        );
        apply_u64_default(
            &mut self.event_post_secs,
            "VPSMAN_EVENT_POST_SECS",
            config.timeout.event_post_secs,
        );
        apply_u64_default(
            &mut self.control_deadline_grace_secs,
            "VPSMAN_CONTROL_DEADLINE_GRACE_SECS",
            config.timeout.control_deadline_grace_secs,
        );
        apply_i64_default(
            &mut self.dispatcher_batch,
            "VPSMAN_DISPATCHER_BATCH",
            config.capacity.dispatcher_batch,
        );
        apply_usize_default(
            &mut self.dispatcher_in_flight,
            "VPSMAN_DISPATCHER_IN_FLIGHT",
            config.capacity.dispatcher_in_flight,
        );
        apply_opt_path(
            &mut self.backup_object_store_dir,
            "VPSMAN_BACKUP_OBJECT_STORE_DIR",
            config.storage.backup_object_store_dir.as_deref(),
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
        apply_string_default(
            &mut self.object_region,
            "VPSMAN_OBJECT_REGION",
            config.storage.object_region.as_deref(),
        );
        apply_bool_default(
            &mut self.object_create_bucket,
            "VPSMAN_OBJECT_CREATE_BUCKET",
            config.storage.object_create_bucket,
        );
        if env_absent("VPSMAN_JOB_OUTPUT_ARTIFACT_MIN_BYTES") {
            if let Some(value) = config.api.job_output_artifact_min_bytes {
                self.job_output_artifact_min_bytes = value;
            }
        }
        if env_absent("VPSMAN_ARTIFACT_MAX_BYTES") {
            if let Some(value) = config.api.artifact_max_bytes {
                self.artifact_max_bytes = value;
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

fn apply_u64_default(target: &mut u64, env_name: &str, value: Option<u64>) {
    if env_absent(env_name) {
        if let Some(value) = value {
            *target = value;
        }
    }
}

fn apply_i64_default(target: &mut i64, env_name: &str, value: Option<i64>) {
    if env_absent(env_name) {
        if let Some(value) = value {
            *target = value;
        }
    }
}

fn apply_usize_default(target: &mut usize, env_name: &str, value: Option<usize>) {
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
        version = build_info::release_version(),
        release_tag = ?build_info::release_tag(),
        server_build_number = build_info::server_build_number(),
        "api build metadata"
    );
    reject_api_privilege_verifier_env()?;
    let repo = Repository::connect(args.postgres_url.as_deref(), &args.migrations_dir).await?;
    let (events, _) = broadcast::channel(256);
    let internal_token = required_internal_token(args.internal_token.as_deref())?;
    let gateway = GatewayDispatchClient::new_with_timeouts(
        args.gateway_control_url.clone(),
        Some(internal_token.clone()),
        GatewayClientTimeouts {
            connect: std::time::Duration::from_secs(args.internal_http_connect_secs.clamp(1, 300)),
            write: std::time::Duration::from_secs(args.internal_http_write_secs.clamp(1, 300)),
            read: std::time::Duration::from_secs(
                args.internal_http_read_secs
                    .max(args.dispatch_ack_secs)
                    .clamp(1, 3600),
            ),
        },
    );
    let backup_object_store = build_backup_object_store(&args)?;
    info!(
        backup_kind = backup_object_store.kind(),
        "object store enabled for backup/general artifacts"
    );
    let update_release_policy =
        UpdateReleasePolicy::new(args.agent_update_allowed_channels.clone())?;
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
        "agent update release policy configured"
    );
    let state = AppState {
        repo,
        events,
        internal_token: Some(internal_token),
        gateway,
        backup_object_store: Some(backup_object_store),
        update_release_policy,
        fleet_alert_policy,
        job_output_artifact_min_bytes: args.job_output_artifact_min_bytes,
        artifact_max_bytes: args.artifact_max_bytes,
        require_registered_agent_updates: args.require_registered_agent_updates,
        suite_config_path: args.suite_config.clone(),
        dispatcher_config: state::DispatcherRuntimeConfig {
            batch_limit: args.dispatcher_batch.clamp(1, 500),
            in_flight: args.dispatcher_in_flight.clamp(1, 512),
            dispatch_ack_secs: args.dispatch_ack_secs.clamp(1, 3600),
            event_post_secs: args.event_post_secs.clamp(1, 3600),
            internal_http_read_secs: args.internal_http_read_secs.clamp(1, 3600),
            control_deadline_grace_secs: args.control_deadline_grace_secs.clamp(0, 3600),
        },
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
                    "version": build_info::release_version(),
                    "release_tag": build_info::release_tag(),
                    "server_build_number": build_info::server_build_number(),
                    "bind": args.bind.to_string(),
                },
            }),
            actor_id: None,
        })
        .await?;
    backup_upload_sessions::spawn_backup_upload_session_cleanup();
    job_dispatcher::spawn_job_dispatcher(state.clone());
    spawn_system_metric_sampler(state.clone());
    let listener = tokio::net::TcpListener::bind(args.bind)
        .await
        .with_context(|| format!("failed to bind API on {}", args.bind))?;
    info!(bind = %args.bind, "api listening");
    axum::serve(
        listener,
        build_router(state).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}

fn spawn_system_metric_sampler(state: AppState) {
    tokio::spawn(async move {
        let mut ticker = time::interval(std::time::Duration::from_secs(60));
        loop {
            ticker.tick().await;
            if let Err(error) = routes_system::record_system_dashboard_sample(&state).await {
                tracing::warn!(%error, "failed to record system dashboard metric sample");
            }
        }
    });
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
                | "dev-internal-token-change-me-32chars"
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

fn build_backup_object_store(args: &Args) -> Result<BackupObjectStore> {
    if let Some(store) = args
        .backup_object_store_dir
        .clone()
        .filter(|path| !path.as_os_str().is_empty())
        .map(BackupObjectStore::filesystem)
        .transpose()?
    {
        return Ok(store);
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
        return Ok(store);
    }

    BackupObjectStore::filesystem(PathBuf::from(DEFAULT_BACKUP_OBJECT_STORE_DIR))
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
mod tests_postgres_reliability;
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
