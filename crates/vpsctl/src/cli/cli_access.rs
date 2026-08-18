use std::path::PathBuf;

use clap::{Args, Subcommand};

use crate::commands_schedules::ScheduleTriggerKindArg;

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
    #[arg(
        long,
        value_delimiter = ',',
        help = "Comma-separated scopes, for example fleet:read,jobs:read,backups:read,terminal:read,integrations:read,templates:read,schedules:read,config:read,network:read,audit:read,jobs:write,config:write,integrations:write,templates:write,history:write"
    )]
    pub(crate) scopes: Vec<String>,
    #[arg(long, default_value = "VPSMAN_NEW_OPERATOR_PASSWORD")]
    pub(crate) password_env: String,
    #[arg(long, default_value_t = 31_536_000)]
    pub(crate) session_refresh_ttl_secs: u64,
    #[arg(long, default_value_t = false)]
    pub(crate) admin_risk_acknowledged: bool,
    #[arg(long, default_value_t = false)]
    pub(crate) confirmed: bool,
}

#[derive(Debug, Args)]
pub(crate) struct OperatorUpdateCommand {
    #[arg(long)]
    pub(crate) operator_id: String,
    #[arg(long)]
    pub(crate) role: String,
    #[arg(
        long,
        value_delimiter = ',',
        help = "Comma-separated scopes. Leave empty for role defaults."
    )]
    pub(crate) scopes: Vec<String>,
    #[arg(long, default_value_t = 31_536_000)]
    pub(crate) session_refresh_ttl_secs: u64,
    #[arg(long, default_value_t = false)]
    pub(crate) admin_risk_acknowledged: bool,
    #[arg(long, default_value_t = false)]
    pub(crate) confirmed: bool,
}

#[derive(Debug, Args)]
pub(crate) struct OperatorLifecycleCommand {
    #[arg(long)]
    pub(crate) operator_id: String,
    #[arg(long, default_value_t = false)]
    pub(crate) admin_risk_acknowledged: bool,
    #[arg(long, default_value_t = false)]
    pub(crate) confirmed: bool,
}

#[derive(Debug, Args)]
pub(crate) struct OperatorPasswordResetCommand {
    #[arg(long)]
    pub(crate) operator_id: String,
    #[arg(long, default_value = "VPSMAN_NEW_OPERATOR_PASSWORD")]
    pub(crate) password_env: String,
    #[arg(long, default_value_t = false)]
    pub(crate) admin_risk_acknowledged: bool,
    #[arg(long, default_value_t = false)]
    pub(crate) confirmed: bool,
}

#[derive(Debug, Args)]
pub(crate) struct OperatorAuthEventsCommand {
    #[arg(long, default_value_t = 100)]
    pub(crate) limit: u16,
    #[arg(long)]
    pub(crate) operator_id: Option<String>,
    #[arg(long)]
    pub(crate) username: Option<String>,
    #[arg(long)]
    pub(crate) result: Option<String>,
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
    #[arg(long, default_value_t = false)]
    pub(crate) admin_risk_acknowledged: bool,
    #[arg(long, default_value_t = false)]
    pub(crate) confirmed: bool,
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
pub(crate) struct AgentIdentityUpsertCommand {
    #[arg(long)]
    pub(crate) client_id: String,
    #[arg(long)]
    pub(crate) client_public_key_hex: String,
    #[arg(long)]
    pub(crate) display_name: Option<String>,
    #[arg(long, value_delimiter = ',')]
    pub(crate) tags: Vec<String>,
    #[arg(long, default_value_t = false)]
    pub(crate) replace_existing_key: bool,
    #[arg(long, default_value_t = false)]
    pub(crate) confirmed: bool,
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
pub(crate) struct ComposeSecretsCommand {
    #[arg(
        long,
        default_value = "config/secrets",
        help = "Directory where Docker Compose secret files are created"
    )]
    pub(crate) secrets_dir: PathBuf,
    #[arg(
        long,
        default_value = "VPSMAN_SUPER_PASSWORD",
        help = "Environment variable containing the local super password"
    )]
    pub(crate) password_env: String,
    #[arg(
        long,
        help = "Existing super-password salt hex; generated and written to operator-privilege.env when omitted"
    )]
    pub(crate) super_salt_hex: Option<String>,
    #[arg(
        long,
        default_value_t = false,
        help = "Replace an existing compose secret set"
    )]
    pub(crate) force: bool,
}

#[derive(Debug, Args)]
pub(crate) struct ConfigPresetListCommand {
    #[arg(
        long,
        help = "Limit to host_metrics, latency_probe, process_inventory, user_sessions, command_execution, or ospf_update_command"
    )]
    pub(crate) behavior: Option<String>,
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
pub(crate) struct VpsRulesCommand {
    #[command(subcommand)]
    pub(crate) command: VpsRulesSubcommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum VpsRulesSubcommand {
    List(VpsRulesListCommand),
    Get(VpsRulesGetCommand),
    Preview(VpsRulesPreviewCommand),
    Upsert(VpsRulesUpsertCommand),
    Unset(VpsRulesUnsetCommand),
}

#[derive(Debug, Args)]
pub(crate) struct VpsRulesListCommand {
    #[arg(long, default_value_t = 50)]
    pub(crate) limit: u16,
    #[arg(long)]
    pub(crate) selector: Option<String>,
    #[arg(long)]
    pub(crate) client_id: Option<String>,
    #[arg(long)]
    pub(crate) key: Option<String>,
    #[arg(long)]
    pub(crate) state: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct VpsRulesGetCommand {
    #[arg(long)]
    pub(crate) client_id: String,
}

#[derive(Debug, Args)]
pub(crate) struct VpsRulesPreviewCommand {
    #[arg(long)]
    pub(crate) selector: String,
    #[arg(long = "set")]
    pub(crate) set_values: Vec<String>,
}

#[derive(Debug, Args)]
pub(crate) struct VpsRulesUpsertCommand {
    #[arg(long)]
    pub(crate) selector: String,
    #[arg(long = "set")]
    pub(crate) set_values: Vec<String>,
    #[arg(long, default_value_t = false)]
    pub(crate) confirmed: bool,
}

#[derive(Debug, Args)]
pub(crate) struct VpsRulesUnsetCommand {
    #[arg(long)]
    pub(crate) selector: String,
    #[arg(long = "key")]
    pub(crate) keys: Vec<String>,
    #[arg(long, default_value_t = false)]
    pub(crate) confirmed: bool,
}

#[derive(Debug, Args)]
pub(crate) struct AlertPoliciesCommand {
    #[command(subcommand)]
    pub(crate) command: AlertPoliciesSubcommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum AlertPoliciesSubcommand {
    List(AlertPoliciesListCommand),
}

#[derive(Debug, Args)]
pub(crate) struct AlertPoliciesListCommand {
    #[arg(long, default_value_t = 50)]
    pub(crate) limit: u16,
    #[arg(long)]
    pub(crate) enabled: Option<bool>,
    #[arg(long)]
    pub(crate) selector: Option<String>,
    #[arg(long)]
    pub(crate) client_id: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct AlertPolicyCommand {
    #[command(subcommand)]
    pub(crate) command: AlertPolicySubcommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum AlertPolicySubcommand {
    Get(AlertPolicyGetCommand),
    Preview(AlertPolicyPreviewCommand),
    Upsert(AlertPolicyUpsertCommand),
}

#[derive(Debug, Args)]
pub(crate) struct AlertPolicyGetCommand {
    #[arg(long)]
    pub(crate) name: String,
}

#[derive(Debug, Args)]
pub(crate) struct AlertPolicyPreviewCommand {
    #[arg(long)]
    pub(crate) name: String,
    #[arg(long)]
    pub(crate) selector: String,
    #[arg(long = "rule-json", value_name = "POLICY_RULE_REQUEST_JSON")]
    pub(crate) rule_json: Vec<String>,
    #[arg(long, default_value_t = true)]
    pub(crate) enabled: bool,
    #[arg(long)]
    pub(crate) notes: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct AlertPolicyUpsertCommand {
    #[arg(long)]
    pub(crate) name: String,
    #[arg(long)]
    pub(crate) selector: Option<String>,
    #[arg(long = "rule-json", value_name = "POLICY_RULE_REQUEST_JSON")]
    pub(crate) rule_json: Vec<String>,
    #[arg(long, default_value_t = true)]
    pub(crate) enabled: bool,
    #[arg(long)]
    pub(crate) notes: Option<String>,
    #[arg(long)]
    pub(crate) file: Option<PathBuf>,
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
    #[arg(long)]
    pub(crate) preview_hash: Option<String>,
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
    #[arg(long)]
    pub(crate) preview_hash: Option<String>,
    #[arg(long, default_value_t = false)]
    pub(crate) confirmed: bool,
}

#[derive(Debug, Args)]
pub(crate) struct ConfigPresetCreateCommand {
    #[arg(long, help = "Supported agent behavior configured by this preset")]
    pub(crate) behavior: String,
    #[arg(long)]
    pub(crate) name: String,
    #[arg(long)]
    pub(crate) description: Option<String>,
    #[arg(long)]
    pub(crate) definition_json: Option<String>,
    #[arg(long)]
    pub(crate) definition_file: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub(crate) struct ConfigPresetCloneCommand {
    #[arg(long)]
    pub(crate) preset_id: String,
    #[arg(long)]
    pub(crate) name: String,
    #[arg(long)]
    pub(crate) description: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct ConfigPresetPreviewCommand {
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
pub(crate) struct ConfigPresetUpdateCommand {
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
    #[arg(
        long,
        requires = "confirmed",
        help = "Hash returned by the separately reviewed preview"
    )]
    pub(crate) preview_hash: Option<String>,
    #[arg(long, default_value_t = false)]
    pub(crate) confirmed: bool,
}

#[derive(Debug, Args)]
pub(crate) struct ConfigPresetDeleteCommand {
    #[arg(long)]
    pub(crate) preset_id: String,
    #[arg(long, default_value_t = false)]
    pub(crate) confirmed: bool,
}

#[derive(Debug, Args)]
pub(crate) struct ConfigSourcesCommand {
    #[arg(long)]
    pub(crate) client_id: Option<String>,
    #[arg(long, help = "Limit effective sources to one supported behavior")]
    pub(crate) behavior: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct ConfigRenderCommand {
    #[arg(long)]
    pub(crate) client_id: String,
    #[arg(long, default_value = "toml")]
    pub(crate) format: String,
}

#[derive(Debug, Args)]
pub(crate) struct ConfigSourceSetCommand {
    #[arg(long, help = "Supported agent behavior to override")]
    pub(crate) behavior: String,
    #[arg(long)]
    pub(crate) preset_id: String,
    #[arg(
        long,
        help = "Target selector expression, combined with --clients/--tags"
    )]
    pub(crate) selector: Option<String>,
    #[arg(long, value_delimiter = ',', help = "Explicit VPS IDs")]
    pub(crate) clients: Vec<String>,
    #[arg(long, value_delimiter = ',', help = "Target tags or selector tokens")]
    pub(crate) tags: Vec<String>,
    #[arg(
        long,
        requires = "confirmed",
        help = "Hash returned by the separately reviewed preview"
    )]
    pub(crate) preview_hash: Option<String>,
    #[arg(long, default_value_t = false)]
    pub(crate) confirmed: bool,
}

#[derive(Debug, Args)]
pub(crate) struct ConfigSourceResetCommand {
    #[arg(long, help = "Supported agent behavior to return to inheritance")]
    pub(crate) behavior: String,
    #[arg(
        long,
        help = "Target selector expression, combined with --clients/--tags"
    )]
    pub(crate) selector: Option<String>,
    #[arg(long, value_delimiter = ',', help = "Explicit VPS IDs")]
    pub(crate) clients: Vec<String>,
    #[arg(long, value_delimiter = ',', help = "Target tags or selector tokens")]
    pub(crate) tags: Vec<String>,
    #[arg(
        long,
        requires = "confirmed",
        help = "Hash returned by the separately reviewed preview"
    )]
    pub(crate) preview_hash: Option<String>,
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
    #[arg(long, help = "Physical span filter (at least 60, divisible by 60)")]
    pub(crate) bucket_secs: Option<i32>,
    #[arg(long, default_value_t = false)]
    pub(crate) latest: bool,
}

#[derive(Debug, Args)]
pub(crate) struct TelemetryNetworkRatesCommand {
    #[arg(long, default_value_t = 50)]
    pub(crate) limit: u16,
    #[arg(long)]
    pub(crate) client_id: Option<String>,
    #[arg(long)]
    pub(crate) interface: Option<String>,
    #[arg(long, help = "Physical span filter (at least 60, divisible by 60)")]
    pub(crate) bucket_secs: Option<i32>,
    #[arg(long, default_value_t = false)]
    pub(crate) latest: bool,
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
    #[arg(long, default_value_t = false)]
    pub(crate) confirmed: bool,
}

#[derive(Debug, Args)]
pub(crate) struct AgentTagCommand {
    #[arg(long)]
    pub(crate) client_id: String,
    #[arg(long)]
    pub(crate) tag: String,
    #[arg(long, default_value_t = false)]
    pub(crate) confirmed: bool,
}

#[derive(Debug, Args)]
pub(crate) struct BulkResolveCommand {
    #[arg(long, value_delimiter = ',')]
    pub(crate) clients: Vec<String>,
    #[arg(long, value_delimiter = ',')]
    pub(crate) tags: Vec<String>,
}

#[derive(Debug, Args)]
#[command(
    after_help = "Examples:\n  vpsctl schedule-create --name hourly --command /bin/true --tags edge --confirmed\n  vpsctl schedule-create --name traffic-guard --trigger-kind event --event-expression 'alert.triggered && alert.category:traffic' --event-argv-template /usr/local/bin/limit-traffic --event-argv-template '{event.kind}' --event-argv-template '{alert.target_id}' --tags edge --confirmed\n\nOmit every --event-argv-template to dispatch the safe /bin/true no-op. Use a separate alert.resolved expression when recovery needs a different fixed argv program. The Schedule web UI provides authoritative per-edge server preview before saving."
)]
pub(crate) struct ScheduleCreateCommand {
    #[arg(
        long,
        value_enum,
        default_value = "cron",
        help = "Dispatch by five-field cron cadence or by a policy-owned alert lifecycle edge"
    )]
    pub(crate) trigger_kind: ScheduleTriggerKindArg,
    #[arg(
        long,
        help = "Cron schedule executable or command label; omit for event schedules"
    )]
    pub(crate) command: Option<String>,
    #[arg(
        long,
        value_delimiter = ',',
        help = "Exact cron job argv; omit for event schedules"
    )]
    pub(crate) argv: Vec<String>,
    #[arg(long, default_value_t = false, help = "Allocate a PTY for a cron job")]
    pub(crate) pty: bool,
    #[arg(
        long,
        help = "Alert-only filter; every OR branch needs alert.triggered/resolved (requires schedules:write, jobs:write, fleet:read, backups:read)"
    )]
    pub(crate) event_expression: Option<String>,
    #[arg(
        long,
        action = clap::ArgAction::Append,
        allow_hyphen_values = true,
        help = "One exact event argv element; repeat in order. Omit all elements for /bin/true"
    )]
    pub(crate) event_argv_template: Vec<String>,
    #[arg(long)]
    pub(crate) name: String,
    #[arg(long, value_delimiter = ',')]
    pub(crate) clients: Vec<String>,
    #[arg(long, value_delimiter = ',')]
    pub(crate) tags: Vec<String>,
    #[arg(
        long,
        help = "Five-field cron cadence; defaults to '0 * * * *' for cron"
    )]
    pub(crate) cron_expr: Option<String>,
    #[arg(long, help = "Cron timezone; only UTC is accepted")]
    pub(crate) timezone: Option<String>,
    #[arg(long, default_value_t = false)]
    pub(crate) disabled: bool,
    #[arg(long, help = "Cron catch-up policy; defaults to skip_missed")]
    pub(crate) catch_up_policy: Option<String>,
    #[arg(long, help = "Cron catch-up bound; defaults to 1")]
    pub(crate) catch_up_limit: Option<i32>,
    #[arg(long, help = "Cron retry delay in seconds; defaults to 300")]
    pub(crate) retry_delay_secs: Option<i64>,
    #[arg(long, default_value_t = 3)]
    pub(crate) max_failures: i32,
    #[arg(long, default_value_t = false)]
    pub(crate) confirmed: bool,
}

#[derive(Debug, Args)]
#[command(
    after_help = "Event updates require --trigger-kind event and the complete replacement definition. The command fetches and submits the current definition revision, so concurrent changes fail closed. Use the Schedule web UI when you need authoritative per-edge server preview before saving."
)]
pub(crate) struct ScheduleUpdateCommand {
    #[arg(
        long,
        value_enum,
        default_value = "cron",
        help = "Dispatch by five-field cron cadence or by a policy-owned alert lifecycle edge"
    )]
    pub(crate) trigger_kind: ScheduleTriggerKindArg,
    #[arg(long)]
    pub(crate) schedule_id: String,
    #[arg(long)]
    pub(crate) name: String,
    #[arg(
        long,
        help = "Cron schedule executable or command label; omit for event schedules"
    )]
    pub(crate) command: Option<String>,
    #[arg(
        long,
        value_delimiter = ',',
        help = "Exact cron job argv; omit for event schedules"
    )]
    pub(crate) argv: Vec<String>,
    #[arg(long, default_value_t = false, help = "Allocate a PTY for a cron job")]
    pub(crate) pty: bool,
    #[arg(
        long,
        help = "Alert-only filter; every OR branch needs alert.triggered/resolved (requires schedules:write, jobs:write, fleet:read, backups:read)"
    )]
    pub(crate) event_expression: Option<String>,
    #[arg(
        long,
        action = clap::ArgAction::Append,
        allow_hyphen_values = true,
        help = "One exact event argv element; repeat in order. Omit all elements for /bin/true"
    )]
    pub(crate) event_argv_template: Vec<String>,
    #[arg(long, value_delimiter = ',')]
    pub(crate) clients: Vec<String>,
    #[arg(long, value_delimiter = ',')]
    pub(crate) tags: Vec<String>,
    #[arg(
        long,
        help = "Five-field cron cadence; defaults to '0 * * * *' for cron"
    )]
    pub(crate) cron_expr: Option<String>,
    #[arg(long, help = "Cron timezone; only UTC is accepted")]
    pub(crate) timezone: Option<String>,
    #[arg(long, default_value_t = false)]
    pub(crate) disabled: bool,
    #[arg(long, help = "Cron catch-up policy; defaults to skip_missed")]
    pub(crate) catch_up_policy: Option<String>,
    #[arg(long, help = "Cron catch-up bound; defaults to 1")]
    pub(crate) catch_up_limit: Option<i32>,
    #[arg(long, help = "Cron retry delay in seconds; defaults to 300")]
    pub(crate) retry_delay_secs: Option<i64>,
    #[arg(long, default_value_t = 3)]
    pub(crate) max_failures: i32,
    #[arg(long, default_value_t = false)]
    pub(crate) confirmed: bool,
}

#[derive(Debug, Args)]
pub(crate) struct ScheduleMutationCommand {
    #[arg(long)]
    pub(crate) schedule_id: String,
    #[arg(long, default_value_t = false)]
    pub(crate) confirmed: bool,
}

#[derive(Debug, Args)]
pub(crate) struct ScheduleDeferCommand {
    #[arg(long)]
    pub(crate) schedule_id: String,
    #[arg(long)]
    pub(crate) deferred_until: String,
    #[arg(long)]
    pub(crate) reason: Option<String>,
    #[arg(long, default_value_t = false)]
    pub(crate) confirmed: bool,
}
