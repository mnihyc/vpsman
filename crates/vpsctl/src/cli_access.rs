use std::path::PathBuf;

use clap::Args;

#[derive(Debug, Args)]
pub(crate) struct BootstrapCommand {
    #[arg(long)]
    pub(crate) username: String,
    #[arg(long, default_value = "VPSMAN_OPERATOR_PASSWORD")]
    pub(crate) password_env: String,
}

#[derive(Debug, Args)]
pub(crate) struct LoginCommand {
    #[arg(long)]
    pub(crate) username: String,
    #[arg(long, default_value = "VPSMAN_OPERATOR_PASSWORD")]
    pub(crate) password_env: String,
    #[arg(long, env = "VPSMAN_TOTP_CODE")]
    pub(crate) totp_code: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct RefreshCommand {
    #[arg(long, default_value = "VPSMAN_REFRESH_TOKEN")]
    pub(crate) refresh_token_env: String,
}

#[derive(Debug, Args)]
pub(crate) struct OperatorCreateCommand {
    #[arg(long)]
    pub(crate) username: String,
    #[arg(long)]
    pub(crate) role: String,
    #[arg(long, value_delimiter = ',')]
    pub(crate) scopes: Vec<String>,
    #[arg(long, default_value = "VPSMAN_NEW_OPERATOR_PASSWORD")]
    pub(crate) password_env: String,
}

#[derive(Debug, Args)]
pub(crate) struct OperatorSessionsCommand {
    #[arg(long, default_value_t = 50)]
    pub(crate) limit: u16,
}

#[derive(Debug, Args)]
pub(crate) struct OperatorSessionRevokeCommand {
    #[arg(long)]
    pub(crate) session_id: String,
}

#[derive(Debug, Args)]
pub(crate) struct TotpPasswordCommand {
    #[arg(long, default_value = "VPSMAN_OPERATOR_PASSWORD")]
    pub(crate) password_env: String,
}

#[derive(Debug, Args)]
pub(crate) struct TotpConfirmCommand {
    #[arg(long, default_value = "VPSMAN_OPERATOR_PASSWORD")]
    pub(crate) password_env: String,
    #[arg(long, default_value = "VPSMAN_TOTP_CODE")]
    pub(crate) code_env: String,
}

#[derive(Debug, Args)]
pub(crate) struct EnrollmentTokenCreateCommand {
    #[arg(long, default_value_t = 1800)]
    pub(crate) ttl_secs: u64,
    #[arg(long, value_delimiter = ',')]
    pub(crate) default_tags: Vec<String>,
    #[arg(long)]
    pub(crate) default_display_name: Option<String>,
    #[arg(long, default_value_t = true)]
    pub(crate) unmanaged_update_enabled: bool,
    #[arg(long)]
    pub(crate) unmanaged_update_version_url: Option<String>,
    #[arg(long, default_value_t = 86_400)]
    pub(crate) unmanaged_update_interval_secs: u64,
    #[arg(long, default_value_t = 86_400)]
    pub(crate) unmanaged_update_jitter_secs: u64,
    #[arg(long, default_value_t = true)]
    pub(crate) unmanaged_update_activate: bool,
    #[arg(long, default_value_t = true)]
    pub(crate) unmanaged_update_restart_agent: bool,
}

#[derive(Debug, Args)]
pub(crate) struct EnrollmentSettingsUpdateCommand {
    #[arg(long = "settings-file")]
    pub(crate) settings_file: PathBuf,
}

#[derive(Debug, Args)]
pub(crate) struct ReenrollmentTokenCreateCommand {
    #[arg(long)]
    pub(crate) client_id: String,
    #[arg(long, default_value_t = 1800)]
    pub(crate) ttl_secs: u64,
    #[arg(long, value_delimiter = ',')]
    pub(crate) default_tags: Vec<String>,
    #[arg(long)]
    pub(crate) default_display_name: Option<String>,
    #[arg(long, default_value_t = false)]
    pub(crate) confirmed: bool,
    #[arg(long, default_value_t = true)]
    pub(crate) preserve_existing_assignments: bool,
    #[arg(long, default_value_t = true)]
    pub(crate) unmanaged_update_enabled: bool,
    #[arg(long)]
    pub(crate) unmanaged_update_version_url: Option<String>,
    #[arg(long, default_value_t = 86_400)]
    pub(crate) unmanaged_update_interval_secs: u64,
    #[arg(long, default_value_t = 86_400)]
    pub(crate) unmanaged_update_jitter_secs: u64,
    #[arg(long, default_value_t = true)]
    pub(crate) unmanaged_update_activate: bool,
    #[arg(long, default_value_t = true)]
    pub(crate) unmanaged_update_restart_agent: bool,
}

#[derive(Debug, Args)]
pub(crate) struct ClientKeyRevokeCommand {
    #[arg(long)]
    pub(crate) client_id: String,
    #[arg(long)]
    pub(crate) reason: Option<String>,
    #[arg(long, default_value_t = false)]
    pub(crate) confirmed: bool,
}

#[derive(Debug, Args)]
pub(crate) struct EnrollClaimCommand {
    #[arg(long, env = "VPSMAN_ENROLLMENT_TOKEN")]
    pub(crate) token: String,
    #[arg(long)]
    pub(crate) client_public_key_hex: String,
}

#[derive(Debug, Args)]
pub(crate) struct EnrollConfigCommand {
    #[arg(long, env = "VPSMAN_ENROLLMENT_TOKEN")]
    pub(crate) token: String,
    #[arg(long, default_value_t = 30)]
    pub(crate) command_timeout_secs: u64,
    #[arg(long = "output-file")]
    pub(crate) output_file: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub(crate) struct DataSourcePresetListCommand {
    #[arg(long)]
    pub(crate) domain: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct DataSourceStatusCommand {
    #[arg(long)]
    pub(crate) client_id: Option<String>,
    #[arg(long)]
    pub(crate) domain: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct FleetAlertsCommand {
    #[arg(long, default_value_t = 50)]
    pub(crate) limit: u16,
    #[arg(long)]
    pub(crate) client_id: Option<String>,
    #[arg(long)]
    pub(crate) severity: Option<String>,
    #[arg(long)]
    pub(crate) category: Option<String>,
    #[arg(long)]
    pub(crate) operator_state: Option<String>,
    #[arg(long, default_value_t = false)]
    pub(crate) include_muted: bool,
}

#[derive(Debug, Args)]
pub(crate) struct FleetAlertExportCommand {
    #[arg(long, default_value_t = 200)]
    pub(crate) limit: u16,
    #[arg(long)]
    pub(crate) client_id: Option<String>,
    #[arg(long)]
    pub(crate) severity: Option<String>,
    #[arg(long)]
    pub(crate) category: Option<String>,
    #[arg(long)]
    pub(crate) operator_state: Option<String>,
    #[arg(long, default_value_t = true)]
    pub(crate) include_muted: bool,
}

#[derive(Debug, Args)]
pub(crate) struct FleetAlertStatesCommand {
    #[arg(long, default_value_t = 50)]
    pub(crate) limit: u16,
    #[arg(long)]
    pub(crate) state: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct FleetAlertStateUpdateCommand {
    #[arg(long)]
    pub(crate) alert_id: String,
    #[arg(long)]
    pub(crate) action: String,
    #[arg(long)]
    pub(crate) muted_for_secs: Option<i64>,
    #[arg(long)]
    pub(crate) reason: Option<String>,
    #[arg(long, default_value_t = false)]
    pub(crate) confirmed: bool,
}

#[derive(Debug, Args)]
pub(crate) struct FleetAlertPoliciesCommand {
    #[arg(long, default_value_t = 50)]
    pub(crate) limit: u16,
    #[arg(long)]
    pub(crate) enabled: Option<bool>,
    #[arg(long)]
    pub(crate) scope_kind: Option<String>,
    #[arg(long)]
    pub(crate) scope_value: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct FleetAlertPolicyUpsertCommand {
    #[arg(long)]
    pub(crate) name: String,
    #[arg(long)]
    pub(crate) scope_kind: String,
    #[arg(long)]
    pub(crate) scope_value: Option<String>,
    #[arg(long)]
    pub(crate) memory_available_warning_ratio: Option<f64>,
    #[arg(long)]
    pub(crate) memory_available_critical_ratio: Option<f64>,
    #[arg(long)]
    pub(crate) disk_available_warning_ratio: Option<f64>,
    #[arg(long)]
    pub(crate) disk_available_critical_ratio: Option<f64>,
    #[arg(long)]
    pub(crate) cpu_load_warning: Option<f64>,
    #[arg(long)]
    pub(crate) cpu_load_critical: Option<f64>,
    #[arg(long, default_value_t = 0)]
    pub(crate) priority: i32,
    #[arg(long, default_value_t = true)]
    pub(crate) enabled: bool,
    #[arg(long)]
    pub(crate) notes: Option<String>,
    #[arg(long, default_value_t = false)]
    pub(crate) confirmed: bool,
}

#[derive(Debug, Args)]
pub(crate) struct FleetAlertNotificationChannelsCommand {
    #[arg(long, default_value_t = 50)]
    pub(crate) limit: u16,
    #[arg(long)]
    pub(crate) enabled: Option<bool>,
    #[arg(long)]
    pub(crate) scope_kind: Option<String>,
    #[arg(long)]
    pub(crate) scope_value: Option<String>,
    #[arg(long)]
    pub(crate) delivery_kind: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct FleetAlertNotificationChannelUpsertCommand {
    #[arg(long)]
    pub(crate) name: String,
    #[arg(long)]
    pub(crate) scope_kind: String,
    #[arg(long)]
    pub(crate) scope_value: Option<String>,
    #[arg(long)]
    pub(crate) min_severity: Option<String>,
    #[arg(long, value_delimiter = ',')]
    pub(crate) categories: Vec<String>,
    #[arg(long, value_delimiter = ',')]
    pub(crate) operator_states: Vec<String>,
    #[arg(long)]
    pub(crate) delivery_kind: String,
    #[arg(long)]
    pub(crate) target: String,
    #[arg(long)]
    pub(crate) cooldown_secs: Option<i64>,
    #[arg(long, default_value_t = true)]
    pub(crate) enabled: bool,
    #[arg(long)]
    pub(crate) notes: Option<String>,
    #[arg(long, default_value_t = false)]
    pub(crate) confirmed: bool,
}

#[derive(Debug, Args)]
pub(crate) struct FleetAlertNotificationsCommand {
    #[arg(long, default_value_t = 50)]
    pub(crate) limit: u16,
    #[arg(long)]
    pub(crate) channel_id: Option<String>,
    #[arg(long)]
    pub(crate) alert_id: Option<String>,
    #[arg(long)]
    pub(crate) status: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct FleetAlertNotificationDispatchCommand {
    #[arg(long, default_value_t = 200)]
    pub(crate) limit: u16,
    #[arg(long)]
    pub(crate) client_id: Option<String>,
    #[arg(long)]
    pub(crate) severity: Option<String>,
    #[arg(long)]
    pub(crate) category: Option<String>,
    #[arg(long)]
    pub(crate) operator_state: Option<String>,
    #[arg(long, default_value_t = false)]
    pub(crate) include_muted: bool,
    #[arg(long, default_value_t = false)]
    pub(crate) dry_run: bool,
    #[arg(long, default_value_t = false)]
    pub(crate) confirmed: bool,
}

#[derive(Debug, Args)]
pub(crate) struct FleetAlertNotificationProcessCommand {
    #[arg(long, default_value_t = 50)]
    pub(crate) limit: u16,
    #[arg(long)]
    pub(crate) status: Option<String>,
    #[arg(long)]
    pub(crate) delivery_kind: Option<String>,
    #[arg(long, default_value_t = false)]
    pub(crate) dry_run: bool,
    #[arg(long, default_value_t = false)]
    pub(crate) confirmed: bool,
}

#[derive(Debug, Args)]
pub(crate) struct DataSourcePresetCreateCommand {
    #[arg(long)]
    pub(crate) domain: String,
    #[arg(long)]
    pub(crate) name: String,
    #[arg(long, default_value = "shared")]
    pub(crate) scope: String,
    #[arg(long)]
    pub(crate) owner_client_id: Option<String>,
    #[arg(long)]
    pub(crate) description: Option<String>,
    #[arg(long)]
    pub(crate) definition_json: Option<String>,
    #[arg(long)]
    pub(crate) definition_file: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub(crate) struct DataSourcePresetCloneCommand {
    #[arg(long)]
    pub(crate) source_preset_id: String,
    #[arg(long)]
    pub(crate) name: String,
    #[arg(long, default_value = "shared")]
    pub(crate) scope: String,
    #[arg(long)]
    pub(crate) owner_client_id: Option<String>,
    #[arg(long)]
    pub(crate) description: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct DataSourcePresetDiffCommand {
    #[arg(long)]
    pub(crate) preset_id: String,
    #[arg(long)]
    pub(crate) description: Option<String>,
    #[arg(long, default_value_t = false)]
    pub(crate) clear_description: bool,
    #[arg(long)]
    pub(crate) definition_json: Option<String>,
    #[arg(long)]
    pub(crate) definition_file: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub(crate) struct DataSourcePresetTestCommand {
    #[arg(long)]
    pub(crate) preset_id: String,
    #[arg(long)]
    pub(crate) definition_json: Option<String>,
    #[arg(long)]
    pub(crate) definition_file: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub(crate) struct DataSourcePresetUpdateCommand {
    #[arg(long)]
    pub(crate) preset_id: String,
    #[arg(long)]
    pub(crate) description: Option<String>,
    #[arg(long, default_value_t = false)]
    pub(crate) clear_description: bool,
    #[arg(long)]
    pub(crate) definition_json: Option<String>,
    #[arg(long)]
    pub(crate) definition_file: Option<PathBuf>,
    #[arg(long, default_value_t = false)]
    pub(crate) confirmed: bool,
}

#[derive(Debug, Args)]
pub(crate) struct DataSourceAssignmentListCommand {
    #[arg(long)]
    pub(crate) client_id: Option<String>,
    #[arg(long)]
    pub(crate) domain: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct DataSourceHotConfigCommand {
    #[arg(long)]
    pub(crate) client_id: String,
    #[arg(long, default_value = "toml")]
    pub(crate) format: String,
}

#[derive(Debug, Args)]
pub(crate) struct DataSourceHotConfigApplyCommand {
    #[arg(long)]
    pub(crate) client_id: String,
    #[arg(long, default_value = "VPSMAN_SUPER_PASSWORD")]
    pub(crate) password_env: String,
    #[arg(long)]
    pub(crate) super_salt_hex: Option<String>,
    #[arg(long, default_value_t = 300)]
    pub(crate) privilege_ttl_secs: u64,
    #[arg(long, default_value_t = 30)]
    pub(crate) timeout_secs: u64,
    #[arg(long, default_value_t = false)]
    pub(crate) confirmed: bool,
    #[arg(long, default_value_t = false)]
    pub(crate) force_unprivileged: bool,
}

#[derive(Debug, Args)]
pub(crate) struct DataSourcePresetAssignCommand {
    #[arg(long)]
    pub(crate) domain: String,
    #[arg(long)]
    pub(crate) preset_id: String,
    #[arg(long, value_delimiter = ',')]
    pub(crate) clients: Vec<String>,
    #[arg(long, value_delimiter = ',')]
    pub(crate) tags: Vec<String>,
    #[arg(long, default_value_t = false)]
    pub(crate) confirmed: bool,
}

#[derive(Debug, Args)]
pub(crate) struct LimitCommand {
    #[arg(long, default_value_t = 50)]
    pub(crate) limit: u16,
}

#[derive(Debug, Args)]
pub(crate) struct TelemetryRollupsCommand {
    #[arg(long, default_value_t = 50)]
    pub(crate) limit: u16,
    #[arg(long)]
    pub(crate) client_id: Option<String>,
    #[arg(long)]
    pub(crate) bucket_secs: Option<i32>,
}

#[derive(Debug, Args)]
pub(crate) struct TelemetryNetworkRatesCommand {
    #[arg(long, default_value_t = 50)]
    pub(crate) limit: u16,
    #[arg(long)]
    pub(crate) client_id: Option<String>,
    #[arg(long)]
    pub(crate) interface: Option<String>,
    #[arg(long)]
    pub(crate) bucket_secs: Option<i32>,
}

#[derive(Debug, Args)]
pub(crate) struct TelemetryTunnelsCommand {
    #[arg(long, default_value_t = 50)]
    pub(crate) limit: u16,
    #[arg(long)]
    pub(crate) client_id: Option<String>,
    #[arg(long)]
    pub(crate) interface: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct NameCommand {
    #[arg(long)]
    pub(crate) name: String,
}

#[derive(Debug, Args)]
pub(crate) struct AgentTagCommand {
    #[arg(long)]
    pub(crate) client_id: String,
    #[arg(long)]
    pub(crate) tag: String,
}

#[derive(Debug, Args)]
pub(crate) struct BulkResolveCommand {
    #[arg(long, value_delimiter = ',')]
    pub(crate) clients: Vec<String>,
    #[arg(long, value_delimiter = ',')]
    pub(crate) tags: Vec<String>,
}

#[derive(Debug, Args)]
pub(crate) struct ScheduleCreateCommand {
    #[arg(long)]
    pub(crate) name: String,
    #[arg(long)]
    pub(crate) command: String,
    #[arg(long, value_delimiter = ',')]
    pub(crate) argv: Vec<String>,
    #[arg(long, default_value_t = false)]
    pub(crate) pty: bool,
    #[arg(long, value_delimiter = ',')]
    pub(crate) clients: Vec<String>,
    #[arg(long, value_delimiter = ',')]
    pub(crate) tags: Vec<String>,
    #[arg(long, default_value = "0 * * * *")]
    pub(crate) cron_expr: String,
    #[arg(long, default_value_t = false)]
    pub(crate) disabled: bool,
    #[arg(long, default_value = "skip_missed")]
    pub(crate) catch_up_policy: String,
    #[arg(long, default_value_t = 1)]
    pub(crate) catch_up_limit: i32,
    #[arg(long, default_value_t = 300)]
    pub(crate) retry_delay_secs: i64,
    #[arg(long, default_value_t = 3)]
    pub(crate) max_failures: i32,
}

#[derive(Debug, Args)]
pub(crate) struct ScheduleUpdateCommand {
    #[arg(long)]
    pub(crate) schedule_id: String,
    #[arg(long)]
    pub(crate) name: String,
    #[arg(long)]
    pub(crate) command: String,
    #[arg(long, value_delimiter = ',')]
    pub(crate) argv: Vec<String>,
    #[arg(long, default_value_t = false)]
    pub(crate) pty: bool,
    #[arg(long, value_delimiter = ',')]
    pub(crate) clients: Vec<String>,
    #[arg(long, value_delimiter = ',')]
    pub(crate) tags: Vec<String>,
    #[arg(long, default_value = "0 * * * *")]
    pub(crate) cron_expr: String,
    #[arg(long, default_value_t = false)]
    pub(crate) disabled: bool,
    #[arg(long, default_value = "skip_missed")]
    pub(crate) catch_up_policy: String,
    #[arg(long, default_value_t = 1)]
    pub(crate) catch_up_limit: i32,
    #[arg(long, default_value_t = 300)]
    pub(crate) retry_delay_secs: i64,
    #[arg(long, default_value_t = 3)]
    pub(crate) max_failures: i32,
}

#[derive(Debug, Args)]
pub(crate) struct ScheduleMutationCommand {
    #[arg(long)]
    pub(crate) schedule_id: String,
}

#[derive(Debug, Args)]
pub(crate) struct ScheduleDeferCommand {
    #[arg(long)]
    pub(crate) schedule_id: String,
    #[arg(long)]
    pub(crate) deferred_until: String,
    #[arg(long)]
    pub(crate) reason: Option<String>,
}
