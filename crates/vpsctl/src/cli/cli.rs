use std::path::PathBuf;

use clap::{Parser, Subcommand};
use uuid::Uuid;
use vpsman_common::DEFAULT_MAX_JOB_TIMEOUT_SECS;

use crate::cli_access::{
    AgentIdentityUpsertCommand, AgentTagCommand, AlertPoliciesCommand, AlertPolicyCommand,
    BootstrapCommand, BulkResolveCommand, ClientKeyRevokeCommand, ConfigPresetCloneCommand,
    ConfigPresetCreateCommand, ConfigPresetDeleteCommand, ConfigPresetListCommand,
    ConfigPresetPreviewCommand, ConfigPresetUpdateCommand, ConfigRenderCommand,
    ConfigSourceResetCommand, ConfigSourceSetCommand, ConfigSourcesCommand,
    FleetAlertExportCommand, FleetAlertNotificationChannelUpsertCommand,
    FleetAlertNotificationChannelsCommand, FleetAlertNotificationDispatchCommand,
    FleetAlertNotificationProcessCommand, FleetAlertNotificationsCommand,
    FleetAlertStateUpdateCommand, FleetAlertStatesCommand, FleetAlertsCommand, LimitCommand,
    LoginCommand, NameCommand, OperatorAuthEventsCommand, OperatorCreateCommand,
    OperatorLifecycleCommand, OperatorPasswordResetCommand, OperatorSessionRevokeCommand,
    OperatorSessionsCommand, OperatorUpdateCommand, RefreshCommand, ScheduleCreateCommand,
    ScheduleDeferCommand, ScheduleMutationCommand, ScheduleUpdateCommand,
    TelemetryNetworkRatesCommand, TelemetryRollupsCommand, TelemetryTunnelsCommand,
    TotpConfirmCommand, TotpPasswordCommand, VpsRulesCommand,
};
use crate::cli_update::{AgentUpdateReleaseLatestArgs, AgentUpdateReleaseRecordArgs};
use crate::commands_host_management::{
    HostProcessRefreshCommand, HostProcessViewCommand, HostServiceActionCommand,
    HostServiceLogsCommand, HostServiceRefreshCommand, HostServiceViewCommand,
    OsUpdateApplyCommand, OsUpdateCheckCommand, OsUpdatePlanCommand, OsUpdateRefreshCommand,
};
use crate::commands_network::{
    NetworkTrafficImportVnstatCommand, TunnelAllocateCommand, TunnelOspfCostUpdateCommand,
    TunnelOspfStatusRefreshCommand, TunnelPlanCommand, TunnelPlanExportCommand,
    TunnelPlanMutationCommand, TunnelProbeCommand, TunnelSpeedTestCommand, TunnelStatusCommand,
};
use crate::commands_port_forwarding::{
    PortForwardBulkCommand, PortForwardCreateCommand, PortForwardMutationCommand,
    PortForwardResolveCommand, PortForwardUpdateCommand,
};
use crate::commands_terminal::{
    TerminalCloseCommand, TerminalInputCommand, TerminalOpenCommand, TerminalPollCommand,
    TerminalResizeCommand,
};
use crate::output::OutputMode;

#[derive(Debug, Parser)]
#[command(
    name = "vpsctl",
    about = "CLI and VTY shell for vpsman",
    version = concat!(
        env!("VPSMAN_RELEASE_VERSION"),
        " (cli build ",
        env!("VPSMAN_CLI_BUILD_NUMBER"),
        ")"
    )
)]
pub(crate) struct Args {
    #[arg(long, env = "VPSMAN_API_URL", default_value = "http://127.0.0.1:8080")]
    pub(crate) api_url: String,
    #[arg(long, env = "VPSMAN_API_TOKEN")]
    pub(crate) api_token: Option<String>,
    #[arg(
        long,
        value_enum,
        global = true,
        env = "VPSMAN_OUTPUT",
        default_value_t = OutputMode::Raw,
        help = "Normalize command stdout as raw text, compact JSON, or pretty JSON"
    )]
    pub(crate) output: OutputMode,
    #[command(subcommand)]
    pub(crate) command: Command,
}

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    Health,
    Bootstrap(BootstrapCommand),
    Login(LoginCommand),
    Refresh(RefreshCommand),
    Me,
    Operators,
    OperatorCreate(OperatorCreateCommand),
    OperatorUpdate(OperatorUpdateCommand),
    OperatorDisable(OperatorLifecycleCommand),
    OperatorEnable(OperatorLifecycleCommand),
    OperatorDelete(OperatorLifecycleCommand),
    OperatorPasswordReset(OperatorPasswordResetCommand),
    OperatorTotpClear(OperatorLifecycleCommand),
    OperatorSessions(OperatorSessionsCommand),
    OperatorSessionRevoke(OperatorSessionRevokeCommand),
    OperatorAuthEvents(OperatorAuthEventsCommand),
    TotpSetup(TotpPasswordCommand),
    TotpConfirm(TotpConfirmCommand),
    TotpDisable(TotpConfirmCommand),
    AgentIdentityUpsert(AgentIdentityUpsertCommand),
    ClientKeyRevocations(LimitCommand),
    ClientKeyRevoke(ClientKeyRevokeCommand),
    KeyLifecycleReport,
    ComposeSecrets(crate::cli_access::ComposeSecretsCommand),
    Summary,
    Agents,
    FleetAlerts(FleetAlertsCommand),
    FleetAlertExport(FleetAlertExportCommand),
    FleetAlertStates(FleetAlertStatesCommand),
    FleetAlertStateUpdate(FleetAlertStateUpdateCommand),
    VpsRules(VpsRulesCommand),
    AlertPolicies(AlertPoliciesCommand),
    AlertPolicy(AlertPolicyCommand),
    FleetAlertNotificationChannels(FleetAlertNotificationChannelsCommand),
    FleetAlertNotificationChannelUpsert(FleetAlertNotificationChannelUpsertCommand),
    FleetAlertNotifications(FleetAlertNotificationsCommand),
    FleetAlertNotificationDispatch(FleetAlertNotificationDispatchCommand),
    FleetAlertNotificationProcess(FleetAlertNotificationProcessCommand),
    GatewaySessions(LimitCommand),
    TelemetryRollups(TelemetryRollupsCommand),
    TelemetryNetworkRates(TelemetryNetworkRatesCommand),
    TelemetryTunnels(TelemetryTunnelsCommand),
    Tags,
    TagCreate(NameCommand),
    AgentTag(AgentTagCommand),
    /// List immutable system and operator-created configuration presets.
    ConfigPresets(ConfigPresetListCommand),
    /// Create a reusable custom configuration preset.
    ConfigPresetCreate(ConfigPresetCreateCommand),
    /// Clone a system or custom preset into an editable custom preset.
    ConfigPresetClone(ConfigPresetCloneCommand),
    /// Preview a complete custom-preset definition and its affected VPSs.
    ConfigPresetPreview(ConfigPresetPreviewCommand),
    /// Preview or confirm a custom-preset update.
    ConfigPresetUpdate(ConfigPresetUpdateCommand),
    /// Delete an unused custom preset.
    ConfigPresetDelete(ConfigPresetDeleteCommand),
    /// Show each VPS's effective preset, origin, readiness, and runtime sync.
    ConfigSources(ConfigSourcesCommand),
    /// Preview or set an explicit configuration-preset override.
    ConfigSourceSet(ConfigSourceSetCommand),
    /// Preview or remove overrides so VPSs inherit the system default.
    ConfigSourceReset(ConfigSourceResetCommand),
    /// Render one VPS's complete effective agent configuration.
    ConfigRender(ConfigRenderCommand),
    Jobs {
        #[arg(long, default_value_t = 50)]
        limit: u16,
    },
    JobRollouts {
        #[arg(long, default_value_t = 50)]
        limit: u16,
    },
    JobRollout {
        #[arg(long)]
        job_id: String,
    },
    JobRolloutPause {
        #[arg(long)]
        job_id: String,
        #[arg(long)]
        reason: Option<String>,
    },
    JobRolloutResume {
        #[arg(long)]
        job_id: String,
        #[arg(long)]
        reason: Option<String>,
        #[arg(long, default_value_t = false)]
        confirmed: bool,
    },
    JobCancel {
        #[arg(long)]
        job_id: String,
        #[arg(long)]
        reason: Option<String>,
        #[arg(long, default_value_t = false)]
        confirmed: bool,
    },
    Schedules,
    ScheduleCreate(ScheduleCreateCommand),
    ScheduleUpdate(ScheduleUpdateCommand),
    ScheduleEnable(ScheduleMutationCommand),
    ScheduleDisable(ScheduleMutationCommand),
    ScheduleDefer(ScheduleDeferCommand),
    ScheduleApplyNow(ScheduleMutationCommand),
    ScheduleDelete(ScheduleMutationCommand),
    JobCreate {
        #[arg(long)]
        command: String,
        #[arg(long, value_delimiter = ',')]
        argv: Vec<String>,
        #[arg(long, default_value_t = false)]
        pty: bool,
        #[arg(long, value_delimiter = ',')]
        clients: Vec<String>,
        #[arg(long, value_delimiter = ',')]
        tags: Vec<String>,
        #[arg(long, default_value = "VPSMAN_SUPER_PASSWORD")]
        password_env: String,
        #[arg(long)]
        super_salt_hex: Option<String>,
        #[arg(long, default_value_t = 300)]
        privilege_ttl_secs: u64,
        #[arg(long, default_value_t = DEFAULT_MAX_JOB_TIMEOUT_SECS)]
        max_timeout_secs: u64,
        #[arg(long, default_value_t = true)]
        privileged: bool,
        #[arg(long, default_value_t = false)]
        destructive: bool,
        #[arg(long, default_value_t = false)]
        confirmed: bool,
        #[arg(long, default_value_t = false)]
        force_unprivileged: bool,
        #[arg(long = "rollout-canary", value_delimiter = ',')]
        rollout_canary_clients: Vec<String>,
        #[arg(long)]
        rollout_batch_size: Option<u16>,
        #[arg(long)]
        rollout_max_failures: Option<u16>,
        #[arg(long)]
        rollout_batch_delay_secs: Option<u32>,
        #[arg(long, default_value_t = false)]
        rollout_continue_after_canary: bool,
    },
    JobShell {
        #[arg(long)]
        script: Option<String>,
        #[arg(long)]
        script_file: Option<PathBuf>,
        #[arg(long, value_delimiter = ',')]
        clients: Vec<String>,
        #[arg(long, value_delimiter = ',')]
        tags: Vec<String>,
        #[arg(long, default_value = "VPSMAN_SUPER_PASSWORD")]
        password_env: String,
        #[arg(long)]
        super_salt_hex: Option<String>,
        #[arg(long, default_value_t = 300)]
        privilege_ttl_secs: u64,
        #[arg(long, default_value_t = DEFAULT_MAX_JOB_TIMEOUT_SECS)]
        max_timeout_secs: u64,
        #[arg(long, default_value_t = false)]
        confirmed: bool,
    },
    FilePull {
        #[arg(long)]
        path: String,
        #[arg(long, default_value_t = false)]
        follow_symlinks: bool,
        #[arg(long, value_delimiter = ',')]
        clients: Vec<String>,
        #[arg(long, value_delimiter = ',')]
        tags: Vec<String>,
        #[arg(long, default_value = "VPSMAN_SUPER_PASSWORD")]
        password_env: String,
        #[arg(long)]
        super_salt_hex: Option<String>,
        #[arg(long, default_value_t = 300)]
        privilege_ttl_secs: u64,
        #[arg(long, default_value_t = DEFAULT_MAX_JOB_TIMEOUT_SECS)]
        max_timeout_secs: u64,
        #[arg(long, default_value_t = false)]
        confirmed: bool,
    },
    TerminalOpen(TerminalOpenCommand),
    TerminalInput(TerminalInputCommand),
    TerminalPoll(TerminalPollCommand),
    TerminalResize(TerminalResizeCommand),
    TerminalClose(TerminalCloseCommand),
    TerminalSessions {
        #[arg(long, default_value_t = 50)]
        limit: u16,
        #[arg(long)]
        client_id: Option<String>,
        #[arg(long)]
        session_id: Option<String>,
    },
    TerminalReplay {
        #[arg(long)]
        client_id: String,
        #[arg(long)]
        session_id: String,
        #[arg(long)]
        from_seq: Option<u64>,
        #[arg(long, default_value_t = 100)]
        limit: u16,
        #[arg(long, default_value_t = 4 * 1024 * 1024)]
        max_bytes: u32,
        #[arg(long = "output-file")]
        output_file: Option<PathBuf>,
        #[arg(long, default_value_t = false)]
        metadata_only: bool,
    },
    TerminalFollow {
        #[arg(long)]
        client_id: String,
        #[arg(long)]
        session_id: String,
        #[arg(long)]
        from_seq: Option<u64>,
        #[arg(long, default_value_t = 500)]
        interval_ms: u64,
        #[arg(long, default_value_t = 0)]
        max_polls: u32,
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    FilePush {
        #[arg(long)]
        source: PathBuf,
        #[arg(long)]
        path: String,
        #[arg(long, default_value = "0644")]
        mode: String,
        #[arg(long, value_delimiter = ',')]
        clients: Vec<String>,
        #[arg(long, value_delimiter = ',')]
        tags: Vec<String>,
        #[arg(long, default_value = "VPSMAN_SUPER_PASSWORD")]
        password_env: String,
        #[arg(long)]
        super_salt_hex: Option<String>,
        #[arg(long, default_value_t = 300)]
        privilege_ttl_secs: u64,
        #[arg(long, default_value_t = DEFAULT_MAX_JOB_TIMEOUT_SECS)]
        max_timeout_secs: u64,
        #[arg(long, default_value_t = false)]
        confirmed: bool,
    },
    FileTransferUpload {
        #[arg(
            long,
            conflicts_with = "source_artifact_id",
            required_unless_present = "source_artifact_id"
        )]
        source: Option<PathBuf>,
        #[arg(long, conflicts_with = "source")]
        source_artifact_id: Option<uuid::Uuid>,
        #[arg(long)]
        path: String,
        #[arg(long, default_value = "0644")]
        mode: String,
        #[arg(long, value_delimiter = ',')]
        clients: Vec<String>,
        #[arg(long, value_delimiter = ',')]
        tags: Vec<String>,
        #[arg(long, default_value = "VPSMAN_SUPER_PASSWORD")]
        password_env: String,
        #[arg(long)]
        super_salt_hex: Option<String>,
        #[arg(long, default_value_t = 300)]
        privilege_ttl_secs: u64,
        #[arg(long, default_value_t = 60)]
        max_timeout_secs: u64,
        #[arg(long, default_value_t = false)]
        confirmed: bool,
        #[arg(long)]
        session_id: Option<uuid::Uuid>,
        #[arg(long)]
        resume_token: Option<String>,
        #[arg(long, default_value_t = 65_536)]
        chunk_size_bytes: u32,
        #[arg(long, default_value_t = 0)]
        rate_limit_kbps: u32,
        #[arg(long, default_value = "replace")]
        existing_policy: String,
        #[arg(long, default_value_t = 250)]
        poll_interval_ms: u64,
        #[arg(long, default_value_t = 0)]
        max_polls: u32,
        #[arg(long, default_value = "same-offset")]
        multi_target_policy: String,
    },
    FileTransferDownload {
        #[arg(long)]
        path: String,
        #[arg(long, default_value_t = false)]
        follow_symlinks: bool,
        #[arg(long)]
        destination: PathBuf,
        #[arg(long, value_delimiter = ',')]
        clients: Vec<String>,
        #[arg(long, value_delimiter = ',')]
        tags: Vec<String>,
        #[arg(long, default_value = "VPSMAN_SUPER_PASSWORD")]
        password_env: String,
        #[arg(long)]
        super_salt_hex: Option<String>,
        #[arg(long, default_value_t = 300)]
        privilege_ttl_secs: u64,
        #[arg(long, default_value_t = 60)]
        max_timeout_secs: u64,
        #[arg(long, default_value_t = false)]
        confirmed: bool,
        #[arg(long)]
        session_id: Option<uuid::Uuid>,
        #[arg(long)]
        resume_token: Option<String>,
        #[arg(long, default_value_t = 65_536)]
        chunk_size_bytes: u32,
        #[arg(long, default_value_t = 0)]
        rate_limit_kbps: u32,
        #[arg(long, default_value_t = 250)]
        poll_interval_ms: u64,
        #[arg(long, default_value_t = 0)]
        max_polls: u32,
        #[arg(long, default_value = "single-target")]
        multi_target_policy: String,
    },
    FileTransfers {
        #[arg(long, default_value_t = 50)]
        limit: u16,
        #[arg(long)]
        client_id: Option<String>,
        #[arg(long)]
        session_id: Option<String>,
    },
    FileTransferHandoff {
        #[arg(long)]
        client_id: String,
        #[arg(long)]
        session_id: String,
        #[arg(long = "output-file")]
        output_file: Option<PathBuf>,
        #[arg(long, default_value_t = false)]
        confirmed: bool,
    },
    FileTransferSources {
        #[arg(long, default_value_t = 50)]
        limit: u16,
    },
    FileTransferSourceUpload {
        #[arg(long)]
        source: PathBuf,
        #[arg(long)]
        name: Option<String>,
        #[arg(long, default_value_t = false)]
        confirmed: bool,
    },
    FileTransferSourceDownload {
        #[arg(long)]
        artifact_id: String,
        #[arg(long = "output-file")]
        output_file: PathBuf,
    },
    UserSessions {
        #[arg(long, value_delimiter = ',')]
        clients: Vec<String>,
        #[arg(long, value_delimiter = ',')]
        tags: Vec<String>,
        #[arg(long, default_value = "VPSMAN_SUPER_PASSWORD")]
        password_env: String,
        #[arg(long)]
        super_salt_hex: Option<String>,
        #[arg(long, default_value_t = 300)]
        privilege_ttl_secs: u64,
        #[arg(long, default_value_t = DEFAULT_MAX_JOB_TIMEOUT_SECS)]
        max_timeout_secs: u64,
        #[arg(long, default_value_t = false)]
        confirmed: bool,
    },
    ConfigPatch {
        #[arg(long)]
        config_file: PathBuf,
        #[arg(long, value_delimiter = ',')]
        clients: Vec<String>,
        #[arg(long, value_delimiter = ',')]
        tags: Vec<String>,
        #[arg(long, default_value = "VPSMAN_SUPER_PASSWORD")]
        password_env: String,
        #[arg(long)]
        super_salt_hex: Option<String>,
        #[arg(long, default_value_t = 300)]
        privilege_ttl_secs: u64,
        #[arg(long)]
        preview_hash: Option<String>,
        #[arg(long, default_value_t = false)]
        confirmed: bool,
    },
    AgentUpdate {
        #[arg(long)]
        artifact_url: String,
        #[arg(long)]
        sha256_hex: String,
        #[arg(long, value_delimiter = ',')]
        clients: Vec<String>,
        #[arg(long, value_delimiter = ',')]
        tags: Vec<String>,
        #[arg(long, default_value = "VPSMAN_SUPER_PASSWORD")]
        password_env: String,
        #[arg(long)]
        super_salt_hex: Option<String>,
        #[arg(long, default_value_t = 300)]
        privilege_ttl_secs: u64,
        #[arg(long, default_value_t = 300)]
        max_timeout_secs: u64,
        #[arg(long, default_value_t = false)]
        confirmed: bool,
        #[arg(long, default_value_t = false)]
        force_unprivileged: bool,
    },
    AgentUpdateCheck {
        #[arg(long)]
        version_url: Option<String>,
        #[arg(long, default_value_t = false)]
        activate: bool,
        #[arg(long, default_value_t = false)]
        restart_agent: bool,
        #[arg(long, value_delimiter = ',')]
        clients: Vec<String>,
        #[arg(long, value_delimiter = ',')]
        tags: Vec<String>,
        #[arg(long, default_value = "VPSMAN_SUPER_PASSWORD")]
        password_env: String,
        #[arg(long)]
        super_salt_hex: Option<String>,
        #[arg(long, default_value_t = 300)]
        privilege_ttl_secs: u64,
        #[arg(long, default_value_t = 300)]
        max_timeout_secs: u64,
        #[arg(long, default_value_t = false)]
        confirmed: bool,
        #[arg(long, default_value_t = false)]
        force_unprivileged: bool,
    },
    AgentUpdateActivate {
        #[arg(long)]
        staged_sha256_hex: String,
        #[arg(long, value_delimiter = ',')]
        clients: Vec<String>,
        #[arg(long, value_delimiter = ',')]
        tags: Vec<String>,
        #[arg(long, default_value = "VPSMAN_SUPER_PASSWORD")]
        password_env: String,
        #[arg(long)]
        super_salt_hex: Option<String>,
        #[arg(long, default_value_t = 300)]
        privilege_ttl_secs: u64,
        #[arg(long, default_value_t = 60)]
        max_timeout_secs: u64,
        #[arg(long, default_value_t = false)]
        restart_agent: bool,
        #[arg(long, default_value_t = false)]
        force_unprivileged: bool,
        #[arg(long, default_value_t = false)]
        confirmed: bool,
    },
    AgentUpdateRollback {
        #[arg(long)]
        rollback_sha256_hex: Option<String>,
        #[arg(long, value_delimiter = ',')]
        clients: Vec<String>,
        #[arg(long, value_delimiter = ',')]
        tags: Vec<String>,
        #[arg(long, default_value = "VPSMAN_SUPER_PASSWORD")]
        password_env: String,
        #[arg(long)]
        super_salt_hex: Option<String>,
        #[arg(long, default_value_t = 300)]
        privilege_ttl_secs: u64,
        #[arg(long, default_value_t = 60)]
        max_timeout_secs: u64,
        #[arg(long, default_value_t = false)]
        force_unprivileged: bool,
        #[arg(long, default_value_t = false)]
        confirmed: bool,
    },
    AgentUpdateReleaseRecord(AgentUpdateReleaseRecordArgs),
    AgentUpdateReleaseLatest(AgentUpdateReleaseLatestArgs),
    AgentUpdateReleases {
        #[arg(long, default_value_t = 25)]
        limit: u16,
    },
    HostProcessRefresh(HostProcessRefreshCommand),
    HostProcesses(HostProcessViewCommand),
    HostServiceRefresh(HostServiceRefreshCommand),
    HostServices(HostServiceViewCommand),
    HostServiceLogs(HostServiceLogsCommand),
    HostServiceAction(HostServiceActionCommand),
    OsUpdateCheck(OsUpdateCheckCommand),
    OsUpdateRefresh(OsUpdateRefreshCommand),
    OsUpdatePlans,
    OsUpdatePlan(OsUpdatePlanCommand),
    OsUpdateApply(OsUpdateApplyCommand),
    ProcessStart {
        #[arg(long)]
        name: String,
        #[arg(long, required = true)]
        argv: Vec<String>,
        #[arg(long)]
        cwd: Option<String>,
        #[arg(long)]
        env: Vec<String>,
        #[arg(long, default_value = "never")]
        restart_policy: String,
        #[arg(long, default_value_t = 0)]
        restart_max_retries: u16,
        #[arg(long, default_value_t = 5)]
        restart_backoff_secs: u64,
        #[arg(long, default_value_t = 5)]
        graceful_stop_secs: u64,
        #[arg(long)]
        memory_max_bytes: Option<u64>,
        #[arg(long)]
        pids_max: Option<u32>,
        #[arg(long)]
        open_files_max: Option<u64>,
        #[arg(long)]
        cpu_shares: Option<u32>,
        #[arg(long, default_value_t = false)]
        no_new_privileges: bool,
        #[arg(long, value_delimiter = ',')]
        clients: Vec<String>,
        #[arg(long, value_delimiter = ',')]
        tags: Vec<String>,
        #[arg(long, default_value = "VPSMAN_SUPER_PASSWORD")]
        password_env: String,
        #[arg(long)]
        super_salt_hex: Option<String>,
        #[arg(long, default_value_t = 300)]
        privilege_ttl_secs: u64,
        #[arg(long, default_value_t = DEFAULT_MAX_JOB_TIMEOUT_SECS)]
        max_timeout_secs: u64,
        #[arg(long, default_value_t = false)]
        confirmed: bool,
        #[arg(long, default_value_t = false)]
        force_unprivileged: bool,
    },
    ProcessStop {
        #[arg(long)]
        name: String,
        #[arg(long, value_delimiter = ',')]
        clients: Vec<String>,
        #[arg(long, value_delimiter = ',')]
        tags: Vec<String>,
        #[arg(long, default_value = "VPSMAN_SUPER_PASSWORD")]
        password_env: String,
        #[arg(long)]
        super_salt_hex: Option<String>,
        #[arg(long, default_value_t = 300)]
        privilege_ttl_secs: u64,
        #[arg(long, default_value_t = DEFAULT_MAX_JOB_TIMEOUT_SECS)]
        max_timeout_secs: u64,
        #[arg(long, default_value_t = false)]
        confirmed: bool,
    },
    ProcessRestart {
        #[arg(long)]
        name: String,
        #[arg(long, value_delimiter = ',')]
        clients: Vec<String>,
        #[arg(long, value_delimiter = ',')]
        tags: Vec<String>,
        #[arg(long, default_value = "VPSMAN_SUPER_PASSWORD")]
        password_env: String,
        #[arg(long)]
        super_salt_hex: Option<String>,
        #[arg(long, default_value_t = 300)]
        privilege_ttl_secs: u64,
        #[arg(long, default_value_t = DEFAULT_MAX_JOB_TIMEOUT_SECS)]
        max_timeout_secs: u64,
        #[arg(long, default_value_t = false)]
        confirmed: bool,
    },
    ProcessStatus {
        #[arg(long)]
        name: Option<String>,
        #[arg(long, value_delimiter = ',')]
        clients: Vec<String>,
        #[arg(long, value_delimiter = ',')]
        tags: Vec<String>,
        #[arg(long, default_value = "VPSMAN_SUPER_PASSWORD")]
        password_env: String,
        #[arg(long)]
        super_salt_hex: Option<String>,
        #[arg(long, default_value_t = 300)]
        privilege_ttl_secs: u64,
        #[arg(long, default_value_t = DEFAULT_MAX_JOB_TIMEOUT_SECS)]
        max_timeout_secs: u64,
        #[arg(long, default_value_t = false)]
        confirmed: bool,
    },
    ProcessLogs {
        #[arg(long)]
        name: String,
        #[arg(long, default_value_t = 65536)]
        max_bytes: u32,
        #[arg(long, value_delimiter = ',')]
        clients: Vec<String>,
        #[arg(long, value_delimiter = ',')]
        tags: Vec<String>,
        #[arg(long, default_value = "VPSMAN_SUPER_PASSWORD")]
        password_env: String,
        #[arg(long)]
        super_salt_hex: Option<String>,
        #[arg(long, default_value_t = 300)]
        privilege_ttl_secs: u64,
        #[arg(long, default_value_t = DEFAULT_MAX_JOB_TIMEOUT_SECS)]
        max_timeout_secs: u64,
        #[arg(long, default_value_t = false)]
        confirmed: bool,
    },
    ProcessSupervisorInventory {
        #[arg(long, default_value_t = 50)]
        limit: u16,
    },
    HostStorageRefresh {
        #[arg(long, default_value_t = 2048)]
        limit: u16,
        #[arg(long, default_value_t = false)]
        include_system_mounts: bool,
        #[arg(long, value_delimiter = ',')]
        clients: Vec<String>,
        #[arg(long, value_delimiter = ',')]
        tags: Vec<String>,
        #[arg(long, default_value_t = 60)]
        max_timeout_secs: u64,
    },
    HostStorage {
        #[arg(long)]
        client_id: String,
        #[arg(long, default_value_t = 2048)]
        limit: u16,
    },
    JobTargets {
        #[arg(long)]
        job_id: String,
    },
    JobTargetStatusDownload {
        #[arg(long)]
        job_id: String,
        #[arg(long = "output-file")]
        output_file: PathBuf,
    },
    JobOutputs {
        #[arg(long)]
        job_id: String,
    },
    JobFollow {
        #[arg(long)]
        job_id: String,
        #[arg(long, default_value_t = 1000)]
        interval_ms: u64,
        #[arg(long, default_value_t = 0)]
        max_polls: u32,
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    JobOutputDownload {
        #[arg(long)]
        job_id: String,
        #[arg(long)]
        client_id: String,
        #[arg(long)]
        seq: i32,
        #[arg(long = "output-file")]
        output_file: PathBuf,
    },
    ServerJobs {
        #[arg(long, default_value_t = 50)]
        limit: u16,
    },
    ArtifactCleanupPreview {
        #[arg(long)]
        expression: String,
        #[arg(long, value_delimiter = ',', required = true)]
        domains: Vec<String>,
    },
    ArtifactCleanupCreate {
        #[arg(long)]
        expression: String,
        #[arg(long, value_delimiter = ',', required = true)]
        domains: Vec<String>,
        #[arg(long)]
        preview_hash: String,
        #[arg(long, default_value_t = false)]
        confirmed: bool,
    },
    ServerJobCancel {
        #[arg(long)]
        job_id: String,
        #[arg(long, default_value_t = false)]
        confirmed: bool,
    },
    Audit {
        #[arg(long, default_value_t = 50)]
        limit: u16,
    },
    HistoryRetention,
    HistoryRetentionUpsert {
        #[arg(
            long,
            help = "Retention domain: audit_logs, system_metric_rollups, telemetry_rollups, telemetry_network_rates, traffic_counter_samples, job_outputs, backup_artifacts, network_observations, topology_history, client_status_history, gateway_sessions"
        )]
        domain: String,
        #[arg(long)]
        retention_days: Option<i32>,
        #[arg(long)]
        prune_limit: Option<i32>,
        #[arg(long)]
        enabled: Option<bool>,
        #[arg(long)]
        metadata_only: Option<bool>,
        #[arg(long)]
        export_enabled: Option<bool>,
        #[arg(long)]
        notes: Option<String>,
        #[arg(long, default_value_t = false)]
        clear_notes: bool,
        #[arg(long, default_value_t = false)]
        confirmed: bool,
    },
    HistoryRetentionPrune {
        #[arg(
            long,
            help = "Retention domain: audit_logs, system_metric_rollups, telemetry_rollups, telemetry_network_rates, traffic_counter_samples, job_outputs, backup_artifacts, network_observations, topology_history, client_status_history, gateway_sessions"
        )]
        domain: Option<String>,
        #[arg(long, default_value_t = false)]
        dry_run: bool,
        #[arg(long)]
        metadata_only: Option<bool>,
        #[arg(long)]
        preview_hash: Option<String>,
        #[arg(long, default_value_t = false)]
        confirmed: bool,
    },
    HistoryExport {
        #[arg(long, help = "Comma-separated retention domains to export")]
        domains: Option<String>,
        #[arg(long, default_value_t = 50)]
        limit: u16,
        #[arg(long)]
        client_id: Option<String>,
        #[arg(long)]
        job_id: Option<String>,
    },
    NetworkObservations {
        #[arg(long, default_value_t = 50)]
        limit: u16,
    },
    #[command(name = "network-trends")]
    NetworkTrends {
        #[arg(long, default_value_t = 50)]
        limit: u16,
    },
    #[command(name = "network-ospf-recommendations")]
    NetworkOspfRecommendations {
        #[arg(long, default_value_t = 50)]
        limit: u16,
    },
    #[command(name = "network-ospf-update-plans")]
    NetworkOspfUpdatePlans {
        #[arg(long, default_value_t = 50)]
        limit: u16,
    },
    #[command(name = "topology-graph")]
    TopologyGraph {
        #[arg(long, default_value_t = 50)]
        limit: u16,
    },
    Backups {
        #[arg(long, default_value_t = 50)]
        limit: u16,
    },
    BackupArtifacts {
        #[arg(long, default_value_t = 50)]
        limit: u16,
    },
    BackupPolicies {
        #[arg(long, default_value_t = 200)]
        limit: u16,
        #[arg(long, default_value_t = 0)]
        offset: u32,
    },
    BackupPolicyUpsert {
        /// Existing backup-policy schedule UUID to update; omit to create.
        #[arg(long, value_name = "UUID")]
        schedule_id: Option<Uuid>,
        #[arg(long)]
        name: String,
        #[arg(long, value_delimiter = ',')]
        paths: Vec<String>,
        #[arg(long, default_value_t = false)]
        include_config: bool,
        #[arg(long, default_value_t = false)]
        follow_symlinks: bool,
        /// Continue when a selected root does not exist on a target.
        #[arg(long, default_value_t = false)]
        skip_missing_paths: bool,
        #[arg(long, value_delimiter = ',')]
        clients: Vec<String>,
        #[arg(long, value_delimiter = ',')]
        tags: Vec<String>,
        #[arg(long, default_value = "0 3 * * *")]
        cron_expr: String,
        #[arg(long, default_value_t = false)]
        disabled: bool,
        #[arg(long, default_value = "skip_missed")]
        catch_up_policy: String,
        #[arg(long, default_value_t = 1)]
        catch_up_limit: i32,
        #[arg(long, default_value_t = 300)]
        retry_delay_secs: i64,
        #[arg(long, default_value_t = 3)]
        max_failures: i32,
        /// Retention days; required with --schedule-id.
        #[arg(long)]
        retention_days: Option<i32>,
        /// Minimum retained runs; required with --schedule-id.
        #[arg(long)]
        keep_last: Option<i32>,
        #[arg(long)]
        rotation_generation: Option<String>,
        /// Explicitly clear rotation generation on an existing policy.
        #[arg(long, default_value_t = false, conflicts_with = "rotation_generation")]
        clear_rotation_generation: bool,
        #[arg(long, default_value_t = false)]
        confirmed: bool,
    },
    BackupPolicyPrune {
        #[arg(long)]
        schedule_id: Option<String>,
        #[arg(long, default_value_t = false)]
        dry_run: bool,
        #[arg(long)]
        metadata_only: Option<bool>,
        #[arg(long)]
        preview_hash: Option<String>,
        #[arg(long, default_value_t = false)]
        confirmed: bool,
    },
    RestorePlans {
        #[arg(long, default_value_t = 50)]
        limit: u16,
    },
    MigrationLinks {
        #[arg(long, default_value_t = 50)]
        limit: u16,
    },
    BackupRequest {
        #[arg(long)]
        client_id: String,
        #[arg(long, value_delimiter = ',')]
        paths: Vec<String>,
        #[arg(long, default_value_t = false)]
        include_config: bool,
        #[arg(long, default_value_t = false)]
        follow_symlinks: bool,
        /// Continue when a selected root does not exist on the target.
        #[arg(long, default_value_t = false)]
        skip_missing_paths: bool,
        #[arg(long)]
        note: Option<String>,
        #[arg(long, default_value = "VPSMAN_SUPER_PASSWORD")]
        password_env: String,
        #[arg(long)]
        super_salt_hex: Option<String>,
        #[arg(long, default_value_t = 300)]
        privilege_ttl_secs: u64,
        #[arg(long, default_value_t = false)]
        confirmed: bool,
    },
    BackupRun {
        #[arg(long, value_delimiter = ',')]
        paths: Vec<String>,
        #[arg(long, default_value_t = false)]
        include_config: bool,
        #[arg(long, default_value_t = false)]
        follow_symlinks: bool,
        /// Continue when a selected root does not exist on a target.
        #[arg(long, default_value_t = false)]
        skip_missing_paths: bool,
        #[arg(long, value_delimiter = ',')]
        clients: Vec<String>,
        #[arg(long, value_delimiter = ',')]
        tags: Vec<String>,
        #[arg(long, default_value = "VPSMAN_SUPER_PASSWORD")]
        password_env: String,
        #[arg(long)]
        super_salt_hex: Option<String>,
        #[arg(long, default_value_t = 300)]
        privilege_ttl_secs: u64,
        #[arg(long, default_value_t = 60)]
        max_timeout_secs: u64,
        #[arg(long, default_value_t = false)]
        confirmed: bool,
    },
    BackupArtifactRecord {
        #[arg(long)]
        backup_request_id: String,
        #[arg(long)]
        object_key: String,
        #[arg(long)]
        sha256_hex: String,
        #[arg(long)]
        size_bytes: i64,
        #[arg(long, default_value_t = false)]
        confirmed: bool,
    },
    BackupArtifactUpload {
        #[arg(long)]
        backup_request_id: String,
        #[arg(long)]
        object_key: String,
        #[arg(long)]
        artifact_file: PathBuf,
        #[arg(long, default_value_t = false)]
        confirmed: bool,
    },
    BackupArtifactUploadChunked {
        #[arg(long)]
        backup_request_id: String,
        #[arg(long)]
        object_key: String,
        #[arg(long)]
        artifact_file: PathBuf,
        #[arg(long, default_value_t = 4 * 1024 * 1024)]
        chunk_size_bytes: usize,
        #[arg(long, default_value_t = false)]
        confirmed: bool,
    },
    BackupArtifactHandoff {
        #[arg(long)]
        backup_request_id: String,
        #[arg(long)]
        job_id: Option<String>,
        #[arg(long, default_value_t = false)]
        confirmed: bool,
    },
    RestorePlan {
        #[arg(long)]
        source_backup_request_id: String,
        #[arg(long)]
        target_client_id: String,
        #[arg(long)]
        note: Option<String>,
        #[arg(long, default_value = "VPSMAN_SUPER_PASSWORD")]
        password_env: String,
        #[arg(long)]
        super_salt_hex: Option<String>,
        #[arg(long, default_value_t = 300)]
        privilege_ttl_secs: u64,
        #[arg(long, default_value_t = false)]
        confirmed: bool,
    },
    MigrationLink {
        #[arg(long)]
        restore_plan_id: String,
        #[arg(long)]
        note: Option<String>,
        #[arg(long, default_value = "VPSMAN_SUPER_PASSWORD")]
        password_env: String,
        #[arg(long)]
        super_salt_hex: Option<String>,
        #[arg(long, default_value_t = 300)]
        privilege_ttl_secs: u64,
        #[arg(long, default_value_t = false)]
        confirmed: bool,
    },
    MigrationRun {
        #[arg(long)]
        restore_plan_id: String,
        #[arg(long)]
        archive_transfer_session_id: String,
        #[arg(long)]
        note: Option<String>,
        #[arg(long, default_value = "VPSMAN_SUPER_PASSWORD")]
        password_env: String,
        #[arg(long)]
        super_salt_hex: Option<String>,
        #[arg(long, default_value_t = 300)]
        privilege_ttl_secs: u64,
        #[arg(long, default_value_t = 60)]
        max_timeout_secs: u64,
        #[arg(long, default_value_t = false)]
        confirmed: bool,
        #[arg(long, default_value_t = false)]
        dry_run: bool,
        #[arg(long, default_value_t = false)]
        force_unprivileged: bool,
    },
    RestoreRun {
        #[arg(long)]
        source_backup_request_id: String,
        #[arg(long)]
        target_client_id: String,
        #[arg(long)]
        archive_transfer_session_id: String,
        #[arg(long, default_value = "VPSMAN_SUPER_PASSWORD")]
        password_env: String,
        #[arg(long)]
        super_salt_hex: Option<String>,
        #[arg(long, default_value_t = 300)]
        privilege_ttl_secs: u64,
        #[arg(long, default_value_t = 60)]
        max_timeout_secs: u64,
        #[arg(long, default_value_t = false)]
        confirmed: bool,
        #[arg(long, default_value_t = false)]
        dry_run: bool,
        #[arg(long, default_value_t = false)]
        force_unprivileged: bool,
    },
    RestoreRollback {
        #[arg(long)]
        restore_job_id: String,
        #[arg(long)]
        target_client_id: String,
        #[arg(long, default_value = "VPSMAN_SUPER_PASSWORD")]
        password_env: String,
        #[arg(long)]
        super_salt_hex: Option<String>,
        #[arg(long, default_value_t = 300)]
        privilege_ttl_secs: u64,
        #[arg(long, default_value_t = 60)]
        max_timeout_secs: u64,
        #[arg(long, default_value_t = false)]
        confirmed: bool,
        #[arg(long, default_value_t = false)]
        force_unprivileged: bool,
    },
    BulkResolve(BulkResolveCommand),
    TunnelPlans,
    PortForwards,
    PortForwardCreate(PortForwardCreateCommand),
    PortForwardUpdate(PortForwardUpdateCommand),
    PortForwardEnable(PortForwardMutationCommand),
    PortForwardDisable(PortForwardMutationCommand),
    PortForwardDelete(PortForwardMutationCommand),
    PortForwardForget(PortForwardMutationCommand),
    PortForwardReapply(PortForwardMutationCommand),
    PortForwardResolve(PortForwardResolveCommand),
    PortForwardBulk(PortForwardBulkCommand),
    TunnelAllocate(TunnelAllocateCommand),
    TunnelPlan(Box<TunnelPlanCommand>),
    TunnelPlanExport(TunnelPlanExportCommand),
    TunnelPlanEnable(TunnelPlanMutationCommand),
    TunnelPlanDisable(TunnelPlanMutationCommand),
    TunnelPlanRotateCredentials(TunnelPlanMutationCommand),
    TunnelPlanDelete(TunnelPlanMutationCommand),
    TunnelOspfStatusRefresh(TunnelOspfStatusRefreshCommand),
    TunnelOspfCostUpdate(TunnelOspfCostUpdateCommand),
    TunnelStatus(TunnelStatusCommand),
    NetworkTrafficImportVnstat(NetworkTrafficImportVnstatCommand),
    TunnelProbe(TunnelProbeCommand),
    TunnelSpeedTest(TunnelSpeedTestCommand),
    NoiseKeygen,
    Vty,
}

impl Command {
    pub(crate) fn supports_output_mode(&self) -> bool {
        !matches!(self, Command::Vty)
    }
}

#[cfg(test)]
#[path = "tests_cli.rs"]
mod tests;
