use anyhow::Result;

use crate::{
    cli::Command, commands::CommandContext, commands_auth, commands_enrollment, commands_inventory,
    commands_schedules,
};

pub(crate) fn dispatch(ctx: &CommandContext, command: Command) -> Result<Option<Command>> {
    let api_url = &ctx.api_url;
    let token = ctx.token();
    match command {
        Command::Health => {
            commands_auth::health(api_url)?;
            Ok(None)
        }
        Command::Bootstrap(command) => {
            commands_auth::bootstrap(api_url, command.username, command.password_env)?;
            Ok(None)
        }
        Command::Login(command) => {
            commands_auth::login(
                api_url,
                command.username,
                command.password_env,
                command.totp_code,
            )?;
            Ok(None)
        }
        Command::Refresh(command) => {
            commands_auth::refresh(api_url, command.refresh_token_env)?;
            Ok(None)
        }
        Command::Me => {
            commands_auth::me(api_url, token)?;
            Ok(None)
        }
        Command::Operators => {
            commands_auth::operators(api_url, token)?;
            Ok(None)
        }
        Command::OperatorCreate(command) => {
            commands_auth::operator_create(
                api_url,
                token,
                command.username,
                command.role,
                command.scopes,
                command.password_env,
            )?;
            Ok(None)
        }
        Command::OperatorSessions(command) => {
            commands_auth::operator_sessions(api_url, token, command.limit)?;
            Ok(None)
        }
        Command::OperatorSessionRevoke(command) => {
            commands_auth::operator_session_revoke(api_url, token, command.session_id)?;
            Ok(None)
        }
        Command::TotpSetup(command) => {
            commands_auth::totp_setup(api_url, token, command.password_env)?;
            Ok(None)
        }
        Command::TotpConfirm(command) => {
            commands_auth::totp_confirm(api_url, token, command.password_env, command.code_env)?;
            Ok(None)
        }
        Command::TotpDisable(command) => {
            commands_auth::totp_disable(api_url, token, command.password_env, command.code_env)?;
            Ok(None)
        }
        Command::EnrollmentTokens => {
            commands_enrollment::enrollment_tokens(api_url, token)?;
            Ok(None)
        }
        Command::EnrollmentTokenCreate(command) => {
            commands_enrollment::enrollment_token_create(
                api_url,
                token,
                commands_enrollment::EnrollmentTokenCreateOptions {
                    ttl_secs: command.ttl_secs,
                    default_tags: command.default_tags,
                    default_display_name: command.default_display_name,
                    unmanaged_update_enabled: command.unmanaged_update_enabled,
                    unmanaged_update_version_url: command.unmanaged_update_version_url,
                    unmanaged_update_interval_secs: command.unmanaged_update_interval_secs,
                    unmanaged_update_jitter_secs: command.unmanaged_update_jitter_secs,
                    unmanaged_update_activate: command.unmanaged_update_activate,
                    unmanaged_update_restart_agent: command.unmanaged_update_restart_agent,
                },
            )?;
            Ok(None)
        }
        Command::ReenrollmentTokenCreate(command) => {
            commands_enrollment::reenrollment_token_create(
                api_url,
                token,
                commands_enrollment::ReenrollmentTokenCreateOptions {
                    client_id: command.client_id,
                    ttl_secs: command.ttl_secs,
                    default_tags: command.default_tags,
                    default_display_name: command.default_display_name,
                    confirmed: command.confirmed,
                    preserve_existing_assignments: command.preserve_existing_assignments,
                    unmanaged_update_enabled: command.unmanaged_update_enabled,
                    unmanaged_update_version_url: command.unmanaged_update_version_url,
                    unmanaged_update_interval_secs: command.unmanaged_update_interval_secs,
                    unmanaged_update_jitter_secs: command.unmanaged_update_jitter_secs,
                    unmanaged_update_activate: command.unmanaged_update_activate,
                    unmanaged_update_restart_agent: command.unmanaged_update_restart_agent,
                },
            )?;
            Ok(None)
        }
        Command::ClientKeyRevocations(command) => {
            commands_enrollment::client_key_revocations(api_url, token, command.limit)?;
            Ok(None)
        }
        Command::ClientKeyRevoke(command) => {
            commands_enrollment::client_key_revoke(
                api_url,
                token,
                command.client_id,
                command.reason,
                command.confirmed,
            )?;
            Ok(None)
        }
        Command::KeyLifecycleReport => {
            commands_enrollment::key_lifecycle_report(api_url, token)?;
            Ok(None)
        }
        Command::EnrollClaim(command) => {
            commands_enrollment::enroll_claim(
                api_url,
                command.token,
                command.client_public_key_hex,
            )?;
            Ok(None)
        }
        Command::EnrollConfig(command) => {
            commands_enrollment::enroll_config(
                api_url,
                command.token,
                command.command_timeout_secs,
                command.output_file,
            )?;
            Ok(None)
        }
        Command::Summary => {
            commands_inventory::summary(api_url, token)?;
            Ok(None)
        }
        Command::Agents => {
            commands_inventory::agents(api_url, token)?;
            Ok(None)
        }
        Command::FleetAlerts(command) => {
            commands_inventory::fleet_alerts(
                api_url,
                token,
                commands_inventory::FleetAlertFilterOptions {
                    limit: command.limit,
                    client_id: command.client_id,
                    severity: command.severity,
                    category: command.category,
                    operator_state: command.operator_state,
                    include_muted: command.include_muted,
                },
            )?;
            Ok(None)
        }
        Command::FleetAlertExport(command) => {
            commands_inventory::fleet_alert_export(
                api_url,
                token,
                commands_inventory::FleetAlertFilterOptions {
                    limit: command.limit,
                    client_id: command.client_id,
                    severity: command.severity,
                    category: command.category,
                    operator_state: command.operator_state,
                    include_muted: command.include_muted,
                },
            )?;
            Ok(None)
        }
        Command::FleetAlertStates(command) => {
            commands_inventory::fleet_alert_states(api_url, token, command.limit, command.state)?;
            Ok(None)
        }
        Command::FleetAlertStateUpdate(command) => {
            commands_inventory::fleet_alert_state_update(
                api_url,
                token,
                commands_inventory::FleetAlertStateUpdateOptions {
                    alert_id: command.alert_id,
                    action: command.action,
                    muted_for_secs: command.muted_for_secs,
                    reason: command.reason,
                    confirmed: command.confirmed,
                },
            )?;
            Ok(None)
        }
        Command::FleetAlertPolicies(command) => {
            commands_inventory::fleet_alert_policies(
                api_url,
                token,
                command.limit,
                command.enabled,
                command.scope_kind,
                command.scope_value,
            )?;
            Ok(None)
        }
        Command::FleetAlertPolicyUpsert(command) => {
            commands_inventory::fleet_alert_policy_upsert(
                api_url,
                token,
                commands_inventory::FleetAlertPolicyUpsertOptions {
                    name: command.name,
                    scope_kind: command.scope_kind,
                    scope_value: command.scope_value,
                    memory_available_warning_ratio: command.memory_available_warning_ratio,
                    memory_available_critical_ratio: command.memory_available_critical_ratio,
                    disk_available_warning_ratio: command.disk_available_warning_ratio,
                    disk_available_critical_ratio: command.disk_available_critical_ratio,
                    cpu_load_warning: command.cpu_load_warning,
                    cpu_load_critical: command.cpu_load_critical,
                    priority: command.priority,
                    enabled: command.enabled,
                    notes: command.notes,
                    confirmed: command.confirmed,
                },
            )?;
            Ok(None)
        }
        Command::FleetAlertNotificationChannels(command) => {
            commands_inventory::fleet_alert_notification_channels(
                api_url,
                token,
                command.limit,
                command.enabled,
                command.scope_kind,
                command.scope_value,
                command.delivery_kind,
            )?;
            Ok(None)
        }
        Command::FleetAlertNotificationChannelUpsert(command) => {
            commands_inventory::fleet_alert_notification_channel_upsert(
                api_url,
                token,
                commands_inventory::FleetAlertNotificationChannelUpsertOptions {
                    name: command.name,
                    scope_kind: command.scope_kind,
                    scope_value: command.scope_value,
                    min_severity: command.min_severity,
                    categories: command.categories,
                    operator_states: command.operator_states,
                    delivery_kind: command.delivery_kind,
                    target: command.target,
                    cooldown_secs: command.cooldown_secs,
                    enabled: command.enabled,
                    notes: command.notes,
                    confirmed: command.confirmed,
                },
            )?;
            Ok(None)
        }
        Command::FleetAlertNotifications(command) => {
            commands_inventory::fleet_alert_notifications(
                api_url,
                token,
                command.limit,
                command.channel_id,
                command.alert_id,
                command.status,
            )?;
            Ok(None)
        }
        Command::FleetAlertNotificationDispatch(command) => {
            commands_inventory::fleet_alert_notification_dispatch(
                api_url,
                token,
                commands_inventory::FleetAlertNotificationDispatchOptions {
                    limit: command.limit,
                    client_id: command.client_id,
                    severity: command.severity,
                    category: command.category,
                    operator_state: command.operator_state,
                    include_muted: command.include_muted,
                    dry_run: command.dry_run,
                    confirmed: command.confirmed,
                },
            )?;
            Ok(None)
        }
        Command::FleetAlertNotificationProcess(command) => {
            commands_inventory::fleet_alert_notification_process(
                api_url,
                token,
                commands_inventory::FleetAlertNotificationProcessOptions {
                    limit: command.limit,
                    status: command.status,
                    delivery_kind: command.delivery_kind,
                    dry_run: command.dry_run,
                    confirmed: command.confirmed,
                },
            )?;
            Ok(None)
        }
        Command::GatewaySessions(command) => {
            commands_inventory::gateway_sessions(api_url, token, command.limit)?;
            Ok(None)
        }
        Command::TelemetryRollups(command) => {
            commands_inventory::telemetry_rollups(
                api_url,
                token,
                command.limit,
                command.client_id,
                command.bucket_secs,
            )?;
            Ok(None)
        }
        Command::TelemetryNetworkRates(command) => {
            commands_inventory::telemetry_network_rates(
                api_url,
                token,
                command.limit,
                command.client_id,
                command.interface,
                command.bucket_secs,
            )?;
            Ok(None)
        }
        Command::TelemetryTunnels(command) => {
            commands_inventory::telemetry_tunnels(
                api_url,
                token,
                command.limit,
                command.client_id,
                command.interface,
            )?;
            Ok(None)
        }
        Command::Tags => {
            commands_inventory::tags(api_url, token)?;
            Ok(None)
        }
        Command::TagCreate(command) => {
            commands_inventory::tag_create(api_url, token, command.name)?;
            Ok(None)
        }
        Command::AgentTag(command) => {
            commands_inventory::agent_tag(api_url, token, command.client_id, command.tag)?;
            Ok(None)
        }
        Command::DataSourcePresets(command) => {
            commands_inventory::data_source_presets(api_url, token, command.domain)?;
            Ok(None)
        }
        Command::DataSourcePresetCreate(command) => {
            commands_inventory::data_source_preset_create(
                api_url,
                token,
                commands_inventory::DataSourcePresetCreateOptions {
                    domain: command.domain,
                    name: command.name,
                    scope: command.scope,
                    owner_client_id: command.owner_client_id,
                    description: command.description,
                    definition_json: command.definition_json,
                    definition_file: command.definition_file,
                },
            )?;
            Ok(None)
        }
        Command::DataSourcePresetClone(command) => {
            commands_inventory::data_source_preset_clone(
                api_url,
                token,
                command.source_preset_id,
                command.name,
                command.scope,
                command.owner_client_id,
                command.description,
            )?;
            Ok(None)
        }
        Command::DataSourcePresetDiff(command) => {
            commands_inventory::data_source_preset_diff(
                api_url,
                token,
                command.preset_id,
                command.description,
                command.clear_description,
                command.definition_json,
                command.definition_file,
            )?;
            Ok(None)
        }
        Command::DataSourcePresetTest(command) => {
            commands_inventory::data_source_preset_test(
                api_url,
                token,
                command.preset_id,
                command.definition_json,
                command.definition_file,
            )?;
            Ok(None)
        }
        Command::DataSourcePresetUpdate(command) => {
            commands_inventory::data_source_preset_update(
                api_url,
                token,
                commands_inventory::DataSourcePresetUpdateOptions {
                    preset_id: command.preset_id,
                    description: command.description,
                    clear_description: command.clear_description,
                    definition_json: command.definition_json,
                    definition_file: command.definition_file,
                    confirmed: command.confirmed,
                },
            )?;
            Ok(None)
        }
        Command::DataSourceStatus(command) => {
            commands_inventory::data_source_status(
                api_url,
                token,
                command.client_id,
                command.domain,
            )?;
            Ok(None)
        }
        Command::DataSourceAssignments(command) => {
            commands_inventory::data_source_assignments(
                api_url,
                token,
                command.client_id,
                command.domain,
            )?;
            Ok(None)
        }
        Command::DataSourceHotConfig(command) => {
            commands_inventory::data_source_hot_config(
                api_url,
                token,
                command.client_id,
                command.format,
            )?;
            Ok(None)
        }
        Command::DataSourceHotConfigApply(command) => {
            commands_inventory::data_source_hot_config_apply(
                api_url,
                token,
                command.client_id,
                command.password_env,
                command.super_salt_hex,
                command.privilege_ttl_secs,
                command.timeout_secs,
                command.confirmed,
                command.force_unprivileged,
            )?;
            Ok(None)
        }
        Command::DataSourcePresetAssign(command) => {
            commands_inventory::data_source_preset_assign(
                api_url,
                token,
                commands_inventory::DataSourcePresetAssignOptions {
                    domain: command.domain,
                    preset_id: command.preset_id,
                    clients: command.clients,
                    tags: command.tags,
                    confirmed: command.confirmed,
                },
            )?;
            Ok(None)
        }
        Command::BulkResolve(command) => {
            commands_inventory::bulk_resolve(api_url, token, command.clients, command.tags)?;
            Ok(None)
        }
        Command::Schedules => {
            commands_schedules::schedules(api_url, token)?;
            Ok(None)
        }
        Command::ScheduleCreate(command) => {
            commands_schedules::schedule_create(
                api_url,
                token,
                command.name,
                command.command,
                command.argv,
                command.pty,
                command.clients,
                command.tags,
                command.cron_expr,
                command.disabled,
                command.catch_up_policy,
                command.catch_up_limit,
                command.retry_delay_secs,
                command.max_failures,
            )?;
            Ok(None)
        }
        Command::ScheduleUpdate(command) => {
            commands_schedules::schedule_update(
                api_url,
                token,
                command.schedule_id,
                command.name,
                command.command,
                command.argv,
                command.pty,
                command.clients,
                command.tags,
                command.cron_expr,
                command.disabled,
                command.catch_up_policy,
                command.catch_up_limit,
                command.retry_delay_secs,
                command.max_failures,
            )?;
            Ok(None)
        }
        Command::ScheduleEnable(command) => {
            commands_schedules::schedule_enable(api_url, token, command.schedule_id)?;
            Ok(None)
        }
        Command::ScheduleDisable(command) => {
            commands_schedules::schedule_disable(api_url, token, command.schedule_id)?;
            Ok(None)
        }
        Command::ScheduleDefer(command) => {
            commands_schedules::schedule_defer(
                api_url,
                token,
                command.schedule_id,
                command.deferred_until,
                command.reason,
            )?;
            Ok(None)
        }
        Command::ScheduleApplyNow(command) => {
            commands_schedules::schedule_apply_now(api_url, token, command.schedule_id)?;
            Ok(None)
        }
        Command::ScheduleDelete(command) => {
            commands_schedules::schedule_delete(api_url, token, command.schedule_id)?;
            Ok(None)
        }
        other => Ok(Some(other)),
    }
}
