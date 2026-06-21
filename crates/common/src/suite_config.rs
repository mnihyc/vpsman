use std::{fs, path::Path};

use serde::{Deserialize, Serialize};

use crate::{DEFAULT_MAX_COMMAND_TIMEOUT_SECS, MAX_CONFIGURABLE_COMMAND_TIMEOUT_SECS};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct SuiteConfig {
    pub version: u32,
    pub api: SuiteApiConfig,
    pub gateway: SuiteGatewayConfig,
    pub worker: SuiteWorkerConfig,
    pub database: SuiteDatabaseConfig,
    pub storage: SuiteStorageConfig,
    pub capacity: SuiteCapacityConfig,
    pub timeout: SuiteTimeoutConfig,
    pub secrets: SuiteSecretRefs,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct SuiteApiConfig {
    pub bind: Option<String>,
    pub gateway_control_url: Option<String>,
    pub require_registered_agent_updates: Option<bool>,
    pub job_output_artifact_min_bytes: Option<usize>,
    pub artifact_max_bytes: Option<usize>,
    pub alert_memory_available_warning_ratio: Option<f64>,
    pub alert_memory_available_critical_ratio: Option<f64>,
    pub alert_disk_available_warning_ratio: Option<f64>,
    pub alert_disk_available_critical_ratio: Option<f64>,
    pub alert_cpu_load_warning: Option<f64>,
    pub alert_cpu_load_critical: Option<f64>,
    pub trusted_proxy_cidrs: Option<Vec<String>>,
    pub operator_auth_username_failed_attempt_limit: Option<i64>,
    pub operator_auth_ip_failed_attempt_limit: Option<i64>,
    pub operator_auth_failed_attempt_window_secs: Option<u64>,
    pub operator_auth_lockout_secs: Option<u64>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct SuiteGatewayConfig {
    pub bind: Option<String>,
    pub control_bind: Option<String>,
    pub api_url: Option<String>,
    pub gateway_id: Option<String>,
    pub reconnect_grace_secs: Option<u64>,
    pub expect_client_public_key_hex: Option<String>,
    pub spool_dir: Option<String>,
    pub spool_ram_max_bytes: Option<u64>,
    pub spool_disk_max_bytes: Option<u64>,
    pub spool_shutdown_flush_secs: Option<u64>,
    pub command_output_event_ttl_secs: Option<u64>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct SuiteWorkerConfig {
    pub tick_secs: Option<u64>,
    pub once: Option<bool>,
    pub worker_id: Option<String>,
    pub worker_lease_secs: Option<i32>,
    pub agent_offline_timeout_secs: Option<i64>,
    pub notification_delivery_limit: Option<i64>,
    pub notification_retention_days: Option<i64>,
    pub notification_retention_prune_limit: Option<i64>,
    pub notification_webhook_timeout_secs: Option<u64>,
    pub webhook_rule_delivery_limit: Option<i64>,
    pub webhook_rule_materialize_limit: Option<i64>,
    pub webhook_rule_retention_days: Option<i64>,
    pub webhook_rule_retention_prune_limit: Option<i64>,
    pub webhook_rule_timeout_secs: Option<u64>,
    pub backup_policy_prune_enabled: Option<bool>,
    pub backup_policy_prune_limit: Option<i64>,
    pub backup_policy_prune_dry_run: Option<bool>,
    pub backup_policy_prune_include_disabled: Option<bool>,
    pub backup_policy_prune_delete_objects: Option<bool>,
    pub backup_policy_prune_object_store_dir: Option<String>,
    pub schedule_command_timeout_secs: Option<u64>,
    pub require_registered_agent_updates: Option<bool>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct SuiteDatabaseConfig {
    pub postgres_url: Option<String>,
    pub migrations_dir: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct SuiteStorageConfig {
    pub backup_object_store_dir: Option<String>,
    pub object_endpoint: Option<String>,
    pub object_bucket: Option<String>,
    pub object_region: Option<String>,
    pub object_create_bucket: Option<bool>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct SuiteCapacityConfig {
    pub api_db_pool: Option<u32>,
    pub worker_db_pool: Option<u32>,
    pub dispatcher_batch: Option<i64>,
    pub dispatcher_in_flight: Option<usize>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct SuiteTimeoutConfig {
    pub max_command_timeout_secs: Option<u64>,
    pub worker_schedule_command_secs: Option<u64>,
    pub agent_offline_secs: Option<i64>,
    pub gateway_reconnect_grace_secs: Option<u64>,
    pub internal_http_connect_secs: Option<u64>,
    pub internal_http_write_secs: Option<u64>,
    pub internal_http_read_secs: Option<u64>,
    pub dispatch_ack_secs: Option<u64>,
    pub event_post_secs: Option<u64>,
    pub control_deadline_grace_secs: Option<u64>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct SuiteSecretRefs {
    pub internal_token_file: Option<String>,
    pub gateway_private_key_file: Option<String>,
    pub privilege_verifier_key_file: Option<String>,
    pub object_access_key_file: Option<String>,
    pub object_secret_key_file: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct SuiteConfigValidation {
    pub valid: bool,
    pub version: u32,
    pub restart_required_fields: Vec<String>,
    pub hot_reload_fields: Vec<String>,
}

impl SuiteConfig {
    pub fn load_optional(path: &Path) -> Result<Self, String> {
        if !path.exists() {
            return Ok(Self {
                version: 1,
                ..Self::default()
            });
        }
        let text = fs::read_to_string(path)
            .map_err(|error| format!("suite_config_read_failed:{error}"))?;
        Self::parse(&text)
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let config: Self =
            toml::from_str(text).map_err(|error| format!("suite_config_invalid_toml:{error}"))?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.version != 1 {
            return Err("suite_config_version_unsupported".to_string());
        }
        if let Some(value) = self.capacity.api_db_pool {
            validate_u32_range(value, 1, 256, "capacity.api_db_pool")?;
        }
        if let Some(value) = self.capacity.worker_db_pool {
            validate_u32_range(value, 1, 256, "capacity.worker_db_pool")?;
        }
        if let Some(value) = self.capacity.dispatcher_batch {
            validate_i64_range(value, 1, 500, "capacity.dispatcher_batch")?;
        }
        if let Some(value) = self.capacity.dispatcher_in_flight {
            if !(1..=512).contains(&value) {
                return Err("capacity.dispatcher_in_flight_out_of_range".to_string());
            }
        }
        validate_optional_u64(self.worker.tick_secs, 1, 3600, "worker.tick_secs")?;
        validate_optional_u64(
            self.timeout.max_command_timeout_secs,
            1,
            MAX_CONFIGURABLE_COMMAND_TIMEOUT_SECS,
            "timeout.max_command_timeout_secs",
        )?;
        validate_optional_u64(
            self.worker.schedule_command_timeout_secs,
            1,
            self.timeout
                .max_command_timeout_secs
                .unwrap_or(DEFAULT_MAX_COMMAND_TIMEOUT_SECS),
            "worker.schedule_command_timeout_secs",
        )?;
        validate_optional_u64(
            self.timeout.worker_schedule_command_secs,
            1,
            self.timeout
                .max_command_timeout_secs
                .unwrap_or(DEFAULT_MAX_COMMAND_TIMEOUT_SECS),
            "timeout.worker_schedule_command_secs",
        )?;
        validate_optional_i64(
            self.worker.agent_offline_timeout_secs,
            1,
            86_400,
            "worker.agent_offline_timeout_secs",
        )?;
        validate_optional_i64(
            self.timeout.agent_offline_secs,
            1,
            86_400,
            "timeout.agent_offline_secs",
        )?;
        validate_optional_u64(
            self.gateway.reconnect_grace_secs,
            0,
            3600,
            "gateway.reconnect_grace_secs",
        )?;
        validate_optional_u64(
            self.gateway.command_output_event_ttl_secs,
            300,
            30 * 24 * 60 * 60,
            "gateway.command_output_event_ttl_secs",
        )?;
        validate_optional_u64(
            self.timeout.gateway_reconnect_grace_secs,
            0,
            3600,
            "timeout.gateway_reconnect_grace_secs",
        )?;
        validate_optional_u64(
            self.timeout.internal_http_connect_secs,
            1,
            300,
            "timeout.internal_http_connect_secs",
        )?;
        validate_optional_u64(
            self.timeout.internal_http_write_secs,
            1,
            300,
            "timeout.internal_http_write_secs",
        )?;
        validate_optional_u64(
            self.timeout.internal_http_read_secs,
            1,
            3600,
            "timeout.internal_http_read_secs",
        )?;
        validate_optional_u64(
            self.timeout.dispatch_ack_secs,
            1,
            3600,
            "timeout.dispatch_ack_secs",
        )?;
        validate_optional_u64(
            self.timeout.event_post_secs,
            1,
            3600,
            "timeout.event_post_secs",
        )?;
        validate_optional_u64(
            self.timeout.control_deadline_grace_secs,
            0,
            3600,
            "timeout.control_deadline_grace_secs",
        )?;
        validate_optional_u64(
            self.gateway.spool_ram_max_bytes,
            1024 * 1024,
            16 * 1024 * 1024 * 1024,
            "gateway.spool_ram_max_bytes",
        )?;
        validate_optional_u64(
            self.gateway.spool_disk_max_bytes,
            1024 * 1024,
            1024 * 1024 * 1024 * 1024,
            "gateway.spool_disk_max_bytes",
        )?;
        validate_optional_u64(
            self.gateway.spool_shutdown_flush_secs,
            1,
            3600,
            "gateway.spool_shutdown_flush_secs",
        )?;
        validate_optional_usize(
            self.api.artifact_max_bytes,
            1024 * 1024,
            4 * 1024 * 1024 * 1024,
            "api.artifact_max_bytes",
        )?;
        validate_optional_i64(
            self.api.operator_auth_username_failed_attempt_limit,
            1,
            1000,
            "api.operator_auth_username_failed_attempt_limit",
        )?;
        validate_optional_i64(
            self.api.operator_auth_ip_failed_attempt_limit,
            1,
            1000,
            "api.operator_auth_ip_failed_attempt_limit",
        )?;
        validate_optional_u64(
            self.api.operator_auth_failed_attempt_window_secs,
            60,
            30 * 24 * 60 * 60,
            "api.operator_auth_failed_attempt_window_secs",
        )?;
        validate_optional_u64(
            self.api.operator_auth_lockout_secs,
            60,
            30 * 24 * 60 * 60,
            "api.operator_auth_lockout_secs",
        )?;
        validate_optional_ip_nets(
            self.api.trusted_proxy_cidrs.as_deref(),
            "api.trusted_proxy_cidrs",
        )?;
        Ok(())
    }

    pub fn validation_summary(&self) -> SuiteConfigValidation {
        SuiteConfigValidation {
            valid: true,
            version: self.version,
            restart_required_fields: vec![
                "api.bind".to_string(),
                "api.gateway_control_url".to_string(),
                "gateway.bind".to_string(),
                "gateway.control_bind".to_string(),
                "gateway.api_url".to_string(),
                "gateway.gateway_id".to_string(),
                "gateway.expect_client_public_key_hex".to_string(),
                "gateway.spool_dir".to_string(),
                "gateway.spool_ram_max_bytes".to_string(),
                "gateway.spool_disk_max_bytes".to_string(),
                "gateway.spool_shutdown_flush_secs".to_string(),
                "database.postgres_url".to_string(),
                "database.migrations_dir".to_string(),
                "secrets.*".to_string(),
                "storage.backup_object_store_dir".to_string(),
                "storage.object_endpoint".to_string(),
                "storage.object_bucket".to_string(),
                "storage.object_region".to_string(),
                "storage.object_create_bucket".to_string(),
                "capacity.api_db_pool".to_string(),
                "capacity.worker_db_pool".to_string(),
                "worker.once".to_string(),
                "worker.worker_id".to_string(),
                "timeout.internal_http_connect_secs".to_string(),
                "timeout.internal_http_write_secs".to_string(),
            ],
            hot_reload_fields: vec![
                "capacity.dispatcher_batch".to_string(),
                "capacity.dispatcher_in_flight".to_string(),
                "timeout.dispatch_ack_secs".to_string(),
                "timeout.event_post_secs".to_string(),
                "timeout.internal_http_read_secs".to_string(),
                "timeout.control_deadline_grace_secs".to_string(),
                "gateway.reconnect_grace_secs".to_string(),
                "gateway.command_output_event_ttl_secs".to_string(),
                "timeout.gateway_reconnect_grace_secs".to_string(),
                "timeout.max_command_timeout_secs".to_string(),
                "api.job_output_artifact_min_bytes".to_string(),
                "api.artifact_max_bytes".to_string(),
                "api.require_registered_agent_updates".to_string(),
                "api.trusted_proxy_cidrs".to_string(),
                "api.operator_auth_*".to_string(),
                "worker.schedule_command_timeout_secs".to_string(),
                "worker.tick_secs".to_string(),
                "worker.worker_lease_secs".to_string(),
                "worker.agent_offline_timeout_secs".to_string(),
                "worker.notification_*".to_string(),
                "worker.webhook_rule_*".to_string(),
                "worker.backup_policy_prune_*".to_string(),
                "worker.require_registered_agent_updates".to_string(),
                "timeout.worker_schedule_command_secs".to_string(),
                "timeout.agent_offline_secs".to_string(),
                "api.alert_*".to_string(),
            ],
        }
    }
}

pub fn read_secret_file_ref(path: Option<&str>) -> Result<Option<String>, String> {
    let Some(path) = path.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let value = fs::read_to_string(path)
        .map_err(|error| format!("secret_ref_read_failed:{path}:{error}"))?;
    Ok(Some(value.trim().to_string()))
}

pub fn redact_suite_config_value(value: serde_json::Value) -> serde_json::Value {
    redact_value(None, value)
}

fn redact_value(parent_key: Option<&str>, value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => serde_json::Value::Object(
            map.into_iter()
                .map(|(child_key, child_value)| {
                    let lowered = child_key.to_ascii_lowercase();
                    let next = if should_redact_value(&lowered, &child_value) {
                        serde_json::Value::String("<redacted>".to_string())
                    } else {
                        redact_value(Some(&lowered), child_value)
                    };
                    (child_key, next)
                })
                .collect(),
        ),
        serde_json::Value::Array(values) => serde_json::Value::Array(
            values
                .into_iter()
                .map(|value| redact_value(parent_key, value))
                .collect(),
        ),
        serde_json::Value::String(text)
            if parent_key
                .map(|key| !key.ends_with("_file") && string_contains_url_credentials(&text))
                .unwrap_or(false) =>
        {
            serde_json::Value::String("<redacted>".to_string())
        }
        other => other,
    }
}

fn should_redact_value(lowered_key: &str, value: &serde_json::Value) -> bool {
    if lowered_key.ends_with("_file") {
        return false;
    }
    if lowered_key == "secrets" && value.is_object() {
        return false;
    }
    lowered_key.contains("secret")
        || lowered_key.ends_with("_key")
        || lowered_key.ends_with("_key_hex")
        || lowered_key.contains("token")
        || lowered_key.contains("password")
        || credential_url_key(lowered_key)
        || value
            .as_str()
            .map(string_contains_url_credentials)
            .unwrap_or(false)
}

fn credential_url_key(lowered_key: &str) -> bool {
    matches!(
        lowered_key,
        "postgres_url"
            | "database_url"
            | "db_url"
            | "connection_url"
            | "connection_string"
            | "postgres_dsn"
            | "database_dsn"
            | "db_dsn"
    ) || lowered_key.contains("dsn")
}

fn string_contains_url_credentials(text: &str) -> bool {
    let Some(scheme_end) = text.find("://") else {
        return false;
    };
    let authority = &text[scheme_end + 3..];
    let authority_end = authority.find(['/', '?', '#']).unwrap_or(authority.len());
    authority[..authority_end].contains('@')
}

fn validate_optional_u64(value: Option<u64>, min: u64, max: u64, name: &str) -> Result<(), String> {
    if let Some(value) = value {
        if !(min..=max).contains(&value) {
            return Err(format!("{name}_out_of_range"));
        }
    }
    Ok(())
}

fn validate_optional_i64(value: Option<i64>, min: i64, max: i64, name: &str) -> Result<(), String> {
    if let Some(value) = value {
        validate_i64_range(value, min, max, name)?;
    }
    Ok(())
}

fn validate_optional_usize(
    value: Option<usize>,
    min: usize,
    max: usize,
    name: &str,
) -> Result<(), String> {
    if let Some(value) = value {
        if !(min..=max).contains(&value) {
            return Err(format!("{name}_out_of_range"));
        }
    }
    Ok(())
}

fn validate_optional_ip_nets(values: Option<&[String]>, name: &str) -> Result<(), String> {
    let Some(values) = values else {
        return Ok(());
    };
    if values.len() > 64 {
        return Err(format!("{name}_too_many_entries"));
    }
    for value in values {
        let value = value.trim();
        if value.is_empty() || value.parse::<ipnet::IpNet>().is_err() {
            return Err(format!("{name}_invalid"));
        }
    }
    Ok(())
}

fn validate_u32_range(value: u32, min: u32, max: u32, name: &str) -> Result<(), String> {
    if !(min..=max).contains(&value) {
        return Err(format!("{name}_out_of_range"));
    }
    Ok(())
}

fn validate_i64_range(value: i64, min: i64, max: i64, name: &str) -> Result<(), String> {
    if !(min..=max).contains(&value) {
        return Err(format!("{name}_out_of_range"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{redact_suite_config_value, SuiteConfig};

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
}
