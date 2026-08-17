use std::{net::SocketAddr, path::PathBuf};

#[path = "auth/auth_model.rs"]
mod auth_model;
#[path = "auth/auth_totp.rs"]
mod auth_totp;
#[path = "backup/backup_auto_artifacts.rs"]
mod backup_auto_artifacts;
#[path = "backup/backup_handoff.rs"]
mod backup_handoff;
#[path = "backup/backup_upload_sessions.rs"]
mod backup_upload_sessions;
#[path = "runtime/build_info.rs"]
mod build_info;
#[path = "auth/client_ip.rs"]
mod client_ip;
#[path = "runtime/error.rs"]
mod error;
#[path = "monitoring/fleet_alert_notifications.rs"]
mod fleet_alert_notifications;
#[path = "monitoring/fleet_alerts.rs"]
mod fleet_alerts;
#[path = "runtime/gateway_client.rs"]
mod gateway_client;
#[path = "auth/internal_operator.rs"]
mod internal_operator;
#[path = "jobs/job_dispatcher.rs"]
mod job_dispatcher;
#[path = "jobs/job_files.rs"]
mod job_files;
#[path = "jobs/job_request.rs"]
mod job_request;
#[path = "jobs/job_terminal.rs"]
mod job_terminal;
#[path = "jobs/job_traffic_import.rs"]
mod job_traffic_import;
#[path = "jobs/lifecycle_outcome.rs"]
mod lifecycle_outcome;
#[path = "model/model.rs"]
mod model;
#[path = "model/model_agent_updates.rs"]
mod model_agent_updates;
#[path = "model/model_alert_notifications.rs"]
mod model_alert_notifications;
#[path = "model/model_alert_policies.rs"]
mod model_alert_policies;
#[path = "model/model_alert_states.rs"]
mod model_alert_states;
#[path = "model/model_backups.rs"]
mod model_backups;
#[path = "model/model_command_templates.rs"]
mod model_command_templates;
#[path = "model/model_configuration_presets.rs"]
mod model_configuration_presets;
#[path = "model/model_dashboard.rs"]
mod model_dashboard;
#[path = "model/model_file_transfer.rs"]
mod model_file_transfer;
#[path = "model/model_fleet_snapshot.rs"]
mod model_fleet_snapshot;
#[path = "model/model_history.rs"]
mod model_history;
#[path = "model/model_home_snapshot.rs"]
mod model_home_snapshot;
#[path = "model/model_host_management.rs"]
mod model_host_management;
#[path = "model/model_monitoring.rs"]
mod model_monitoring;
#[path = "model/model_port_forwarding.rs"]
mod model_port_forwarding;
#[path = "model/model_runtime_config.rs"]
mod model_runtime_config;
#[path = "model/model_server_jobs.rs"]
mod model_server_jobs;
#[path = "model/model_terminal.rs"]
mod model_terminal;
#[path = "model/model_topology.rs"]
mod model_topology;
#[path = "model/model_webhook_rules.rs"]
mod model_webhook_rules;
#[path = "monitoring/network_ospf_controller.rs"]
mod network_ospf_controller;
pub(crate) mod object_store {
    pub(crate) use vpsman_object_store::*;
}
#[path = "auth/privilege.rs"]
mod privilege;
#[path = "repository/core/repository.rs"]
mod repository;
#[path = "repository/system/repository_agent_update_lifecycle.rs"]
mod repository_agent_update_lifecycle;
#[path = "repository/system/repository_agent_update_releases.rs"]
mod repository_agent_update_releases;
#[path = "repository/fleet/repository_alert_notifications.rs"]
mod repository_alert_notifications;
#[path = "repository/fleet/repository_alert_policies.rs"]
mod repository_alert_policies;
#[path = "repository/fleet/repository_alert_states.rs"]
mod repository_alert_states;
#[path = "repository/access/repository_auth.rs"]
mod repository_auth;
#[path = "repository/backup/repository_backup_artifacts.rs"]
mod repository_backup_artifacts;
#[path = "repository/backup/repository_backup_policies.rs"]
mod repository_backup_policies;
#[path = "repository/backup/repository_backups.rs"]
mod repository_backups;
#[path = "repository/config/repository_command_templates.rs"]
mod repository_command_templates;
#[path = "repository/config/repository_configuration_presets.rs"]
mod repository_configuration_presets;
#[path = "repository/jobs/repository_file_transfer_sources.rs"]
mod repository_file_transfer_sources;
#[path = "repository/jobs/repository_file_transfers.rs"]
mod repository_file_transfers;
#[path = "repository/access/repository_gateway_sessions.rs"]
mod repository_gateway_sessions;
#[path = "repository/system/repository_history.rs"]
mod repository_history;
#[path = "repository/system/repository_host_management.rs"]
mod repository_host_management;
#[path = "repository/fleet/repository_ingest.rs"]
mod repository_ingest;
#[path = "repository/fleet/repository_inventory.rs"]
mod repository_inventory;
#[path = "repository/jobs/repository_job_outputs.rs"]
mod repository_job_outputs;
#[path = "repository/jobs/repository_job_rollouts.rs"]
mod repository_job_rollouts;
#[path = "repository/jobs/repository_jobs.rs"]
mod repository_jobs;
#[path = "repository/access/repository_key_lifecycle.rs"]
mod repository_key_lifecycle;
#[path = "repository/core/repository_migrations.rs"]
mod repository_migrations;
#[path = "repository/fleet/repository_monitoring.rs"]
mod repository_monitoring;
#[path = "repository/network/repository_network.rs"]
mod repository_network;
#[path = "repository/network/repository_network_adapters.rs"]
mod repository_network_adapters;
#[path = "repository/network/repository_network_observations.rs"]
mod repository_network_observations;
#[path = "repository/network/repository_network_recommendations.rs"]
mod repository_network_recommendations;
#[path = "repository/network/repository_network_traffic_import.rs"]
mod repository_network_traffic_import;
#[path = "repository/access/repository_operator_totp.rs"]
mod repository_operator_totp;
#[path = "repository/network/repository_port_forwarding.rs"]
mod repository_port_forwarding;
#[path = "repository/backup/repository_restores.rs"]
mod repository_restores;
#[path = "repository/config/repository_runtime_config.rs"]
mod repository_runtime_config;
#[path = "repository/config/repository_runtime_config_patch_generators.rs"]
mod repository_runtime_config_patch_generators;
#[path = "repository/jobs/repository_schedules.rs"]
mod repository_schedules;
#[path = "repository/jobs/repository_server_jobs.rs"]
mod repository_server_jobs;
#[path = "repository/config/repository_suite_config.rs"]
mod repository_suite_config;
#[path = "repository/system/repository_system_dashboard.rs"]
mod repository_system_dashboard;
#[path = "repository/fleet/repository_telemetry_rollups.rs"]
mod repository_telemetry_rollups;
#[path = "repository/jobs/repository_terminal_sessions.rs"]
mod repository_terminal_sessions;
#[path = "repository/network/repository_topology_graph.rs"]
mod repository_topology_graph;
#[path = "repository/network/repository_tunnel_credentials.rs"]
mod repository_tunnel_credentials;
#[path = "repository/config/repository_webhook_rules.rs"]
mod repository_webhook_rules;
#[path = "routes/core/routes.rs"]
mod routes;
#[path = "routes/fleet/routes_alerts.rs"]
mod routes_alerts;
#[path = "routes/access/routes_auth.rs"]
mod routes_auth;
#[path = "routes/backup/routes_backups.rs"]
mod routes_backups;
#[path = "routes/config/routes_command_templates.rs"]
mod routes_command_templates;
#[path = "routes/config/routes_configuration_presets.rs"]
mod routes_configuration_presets;
#[path = "routes/fleet/routes_dashboard.rs"]
mod routes_dashboard;
#[path = "routes/jobs/routes_file_transfers.rs"]
mod routes_file_transfers;
#[path = "routes/fleet/routes_fleet_snapshot.rs"]
mod routes_fleet_snapshot;
#[path = "routes/fleet/routes_history.rs"]
mod routes_history;
#[path = "routes/fleet/routes_home_snapshot.rs"]
mod routes_home_snapshot;
#[path = "routes/operations/routes_host_management.rs"]
mod routes_host_management;
#[path = "routes/fleet/routes_ingest.rs"]
mod routes_ingest;
#[path = "routes/fleet/routes_inventory.rs"]
mod routes_inventory;
#[path = "routes/jobs/routes_job_history.rs"]
mod routes_job_history;
#[path = "routes/jobs/routes_job_rollouts.rs"]
mod routes_job_rollouts;
#[path = "routes/jobs/routes_jobs.rs"]
mod routes_jobs;
#[path = "routes/access/routes_key_lifecycle.rs"]
mod routes_key_lifecycle;
#[path = "routes/operations/routes_migrations.rs"]
mod routes_migrations;
#[path = "routes/fleet/routes_monitoring.rs"]
mod routes_monitoring;
#[path = "routes/network/routes_network.rs"]
mod routes_network;
#[path = "routes/network/routes_port_forwarding.rs"]
mod routes_port_forwarding;
#[path = "routes/backup/routes_restores.rs"]
mod routes_restores;
#[path = "routes/config/routes_runtime_config_workspace.rs"]
mod routes_runtime_config_workspace;
#[path = "routes/jobs/routes_schedules.rs"]
mod routes_schedules;
#[path = "routes/jobs/routes_server_jobs.rs"]
mod routes_server_jobs;
#[path = "routes/config/routes_suite_config.rs"]
mod routes_suite_config;
#[path = "routes/fleet/routes_system.rs"]
mod routes_system;
#[path = "routes/jobs/routes_terminal_sessions.rs"]
mod routes_terminal_sessions;
#[path = "routes/operations/routes_update_releases.rs"]
mod routes_update_releases;
#[path = "routes/config/routes_webhook_rules.rs"]
mod routes_webhook_rules;
#[path = "routes/core/routes_ws.rs"]
mod routes_ws;
#[path = "runtime/runtime_config.rs"]
mod runtime_config;
#[path = "runtime/runtime_config_workspace.rs"]
mod runtime_config_workspace;
#[path = "auth/security.rs"]
mod security;
#[path = "monitoring/selector_expression.rs"]
mod selector_expression;
#[path = "runtime/state.rs"]
mod state;
#[path = "runtime/util.rs"]
mod util;
#[path = "monitoring/webhook_rules.rs"]
mod webhook_rules;

use anyhow::{Context, Result};
use clap::Parser;
use fleet_alerts::FleetAlertPolicy;
use gateway_client::{GatewayClientTimeouts, GatewayDispatchClient};
use object_store::{BackupObjectStore, S3BackupObjectStoreSettings};
use repository::Repository;
use routes::build_router;
use state::{
    remember_suite_config, AppState, UpdateReleasePolicy, WsEventBus, DEFAULT_ARTIFACT_MAX_BYTES,
};
use tokio::time;
use tracing::info;
use tracing_subscriber::fmt::writer::MakeWriterExt;
use vpsman_common::{
    read_secret_file_ref, SuiteConfig, DEFAULT_MAX_JOB_TIMEOUT_SECS,
    MAX_CONFIGURABLE_JOB_TIMEOUT_SECS,
};

const DEFAULT_BACKUP_OBJECT_STORE_DIR: &str = "runtime/data/objects/backups";
const DEFAULT_POLICY_EVALUATION_INTERVAL_SECS: u64 = 30;

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
        totp_last_accepted_step: None,
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
    #[arg(
        long,
        env = "VPSMAN_MAX_JOB_TIMEOUT_SECS",
        default_value_t = DEFAULT_MAX_JOB_TIMEOUT_SECS
    )]
    max_job_timeout_secs: u64,
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
    #[arg(
        long,
        env = "VPSMAN_POLICY_EVALUATION_INTERVAL_SECS",
        default_value_t = DEFAULT_POLICY_EVALUATION_INTERVAL_SECS
    )]
    policy_evaluation_interval_secs: u64,
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
        apply_u64_default(
            &mut self.max_job_timeout_secs,
            "VPSMAN_MAX_JOB_TIMEOUT_SECS",
            config.timeout.max_job_timeout_secs,
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
        if env_absent("VPSMAN_ALERT_MEMORY_AVAILABLE_WARNING_RATIO")
            && env_absent("VPSMAN_ALERT_MEMORY_AVAILABLE_CRITICAL_RATIO")
        {
            if let (Some(warning), Some(critical)) = (
                config.api.alert_memory_available_warning_ratio,
                config.api.alert_memory_available_critical_ratio,
            ) {
                self.alert_memory_available_warning_ratio = warning;
                self.alert_memory_available_critical_ratio = critical;
            }
        }
        if env_absent("VPSMAN_ALERT_DISK_AVAILABLE_WARNING_RATIO")
            && env_absent("VPSMAN_ALERT_DISK_AVAILABLE_CRITICAL_RATIO")
        {
            if let (Some(warning), Some(critical)) = (
                config.api.alert_disk_available_warning_ratio,
                config.api.alert_disk_available_critical_ratio,
            ) {
                self.alert_disk_available_warning_ratio = warning;
                self.alert_disk_available_critical_ratio = critical;
            }
        }
        if env_absent("VPSMAN_ALERT_CPU_LOAD_WARNING")
            && env_absent("VPSMAN_ALERT_CPU_LOAD_CRITICAL")
        {
            if let (Some(warning), Some(critical)) = (
                config.api.alert_cpu_load_warning,
                config.api.alert_cpu_load_critical,
            ) {
                self.alert_cpu_load_warning = warning;
                self.alert_cpu_load_critical = critical;
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
    let log_writer = std::io::stderr
        .with_max_level(tracing::Level::WARN)
        .or_else(std::io::stdout.with_min_level(tracing::Level::INFO));
    tracing_subscriber::fmt()
        .with_writer(log_writer)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "warn,vpsman_api=info,tower_http=info".into()),
        )
        .init();

    let mut args = Args::parse();
    // Keep the non-suite startup policy as the hot-reload baseline. Suite
    // values are applied on every read, so deleting a suite key restores the
    // CLI/environment/default value instead of retaining a stale suite value.
    let fleet_alert_policy = FleetAlertPolicy::new(
        args.alert_memory_available_warning_ratio,
        args.alert_memory_available_critical_ratio,
        args.alert_disk_available_warning_ratio,
        args.alert_disk_available_critical_ratio,
        args.alert_cpu_load_warning,
        args.alert_cpu_load_critical,
    )?;
    let suite_config =
        SuiteConfig::load_optional(&args.suite_config).map_err(anyhow::Error::msg)?;
    remember_suite_config(&args.suite_config, &suite_config);
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
    let (events, ws_invalidations) = WsEventBus::new(256);
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
    FleetAlertPolicy::new(
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
            max_job_timeout_secs: args
                .max_job_timeout_secs
                .clamp(1, MAX_CONFIGURABLE_JOB_TIMEOUT_SECS),
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
    job_traffic_import::spawn_network_traffic_import_finalizer(state.clone());
    job_dispatcher::spawn_job_dispatcher(state.clone());
    network_ospf_controller::spawn_automatic_ospf_controller(state.clone());
    spawn_policy_evaluator(
        state.repo.clone(),
        args.policy_evaluation_interval_secs.clamp(5, 3600),
    );
    spawn_system_metric_sampler(state.clone());
    let listener = tokio::net::TcpListener::bind(args.bind)
        .await
        .with_context(|| format!("failed to bind API on {}", args.bind))?;
    info!(bind = %args.bind, "api listening");
    let ws_invalidation_task =
        routes_ws::spawn_ws_invalidation_coalescer(state.events.clone(), ws_invalidations);
    let server_result = axum::serve(
        listener,
        build_router(state).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await;
    ws_invalidation_task.abort();
    match ws_invalidation_task.await {
        Err(error) if error.is_cancelled() => {}
        Err(error) => return Err(error).context("WebSocket invalidation task failed"),
        Ok(()) => {}
    }
    server_result?;
    Ok(())
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).ok();
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {},
            _ = async {
                if let Some(signal) = terminate.as_mut() {
                    signal.recv().await;
                } else {
                    std::future::pending::<()>().await;
                }
            } => {},
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
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

fn spawn_policy_evaluator(repo: Repository, interval_secs: u64) {
    tokio::spawn(async move {
        let mut ticker = time::interval(std::time::Duration::from_secs(interval_secs));
        ticker.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            if let Err(error) = repo.evaluate_policy_rules().await {
                tracing::warn!(%error, "failed to evaluate fleet alert policies");
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
#[path = "runtime/tests_main.rs"]
mod tests;
#[cfg(test)]
#[path = "monitoring/tests_alerts.rs"]
mod tests_alerts;
#[cfg(test)]
#[path = "auth/tests_auth.rs"]
mod tests_auth;
#[cfg(test)]
#[path = "backup/tests_backups.rs"]
mod tests_backups;
#[cfg(test)]
#[path = "runtime/tests_config.rs"]
mod tests_config;
#[cfg(test)]
#[path = "monitoring/tests_dashboard.rs"]
mod tests_dashboard;
#[cfg(test)]
#[path = "jobs/tests_files.rs"]
mod tests_files;
#[cfg(test)]
#[path = "monitoring/tests_history.rs"]
mod tests_history;
#[cfg(test)]
#[path = "auth/tests_identity.rs"]
mod tests_identity;
#[cfg(test)]
#[path = "jobs/tests_job_approvals.rs"]
mod tests_job_approvals;
#[cfg(test)]
#[path = "repository/core/tests_migrations.rs"]
mod tests_migrations;
#[cfg(test)]
#[path = "monitoring/tests_monitoring.rs"]
mod tests_monitoring;
#[cfg(test)]
#[path = "monitoring/tests_network.rs"]
mod tests_network;
#[cfg(test)]
#[path = "monitoring/tests_network_observations.rs"]
mod tests_network_observations;
#[cfg(test)]
#[path = "monitoring/tests_network_ospf_updates.rs"]
mod tests_network_ospf_updates;
#[cfg(test)]
#[path = "monitoring/tests_network_telemetry.rs"]
mod tests_network_telemetry;
#[cfg(test)]
#[path = "backup/tests_object_store.rs"]
mod tests_object_store;
#[cfg(test)]
#[path = "monitoring/tests_port_forwarding.rs"]
mod tests_port_forwarding;
#[cfg(test)]
#[path = "repository/core/tests_postgres_reliability.rs"]
mod tests_postgres_reliability;
#[cfg(test)]
#[path = "jobs/tests_process.rs"]
mod tests_process;
#[cfg(test)]
#[path = "backup/tests_restores.rs"]
mod tests_restores;
#[cfg(test)]
#[path = "jobs/tests_rollouts.rs"]
mod tests_rollouts;
#[cfg(test)]
#[path = "jobs/tests_schedules.rs"]
mod tests_schedules;
#[cfg(test)]
#[path = "jobs/tests_terminal.rs"]
mod tests_terminal;
#[cfg(test)]
#[path = "jobs/tests_update_releases.rs"]
mod tests_update_releases;
