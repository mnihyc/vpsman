use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use vpsman_common::{
    parse_and_validate_alert_event_expression, validate_alert_event_argv_template, JobCommand,
};

use crate::{
    http::{http_delete_json, http_get, http_post_json, http_put_json},
    privilege::{
        build_privilege_for_schedule, load_super_password, load_super_salt_hex,
        SchedulePrivilegePayload, SchedulePrivilegeRequest,
    },
};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ScheduleTriggerKindArg {
    Cron,
    Event,
}

impl ScheduleTriggerKindArg {
    fn as_str(self) -> &'static str {
        match self {
            Self::Cron => "cron",
            Self::Event => "event",
        }
    }
}

#[derive(Debug, Deserialize)]
struct ScheduleRecord {
    id: String,
    name: String,
    enabled: bool,
    trigger_kind: ScheduleTriggerKindArg,
    definition_revision: i64,
    command_type: String,
    operation_payload_hash: String,
    selector_expression: String,
    target_client_ids: Vec<String>,
    cron_expr: Option<String>,
    timezone: Option<String>,
    event_expression: Option<String>,
    catch_up_policy: Option<String>,
    catch_up_limit: Option<i32>,
    retry_delay_secs: Option<i64>,
    max_failures: i32,
    deferred_until: Option<String>,
}

pub(crate) struct SavedScheduleTargetSnapshot {
    pub(crate) selector_expression: String,
    pub(crate) target_client_ids: Vec<String>,
    pub(crate) definition_revision: i64,
}

pub(crate) struct ScheduleDefinitionOptions {
    pub(crate) name: String,
    pub(crate) trigger_kind: ScheduleTriggerKindArg,
    pub(crate) command: Option<String>,
    pub(crate) argv: Vec<String>,
    pub(crate) pty: bool,
    pub(crate) event_expression: Option<String>,
    pub(crate) event_argv_template: Vec<String>,
    pub(crate) cron_expr: Option<String>,
    pub(crate) timezone: Option<String>,
    pub(crate) disabled: bool,
    pub(crate) catch_up_policy: Option<String>,
    pub(crate) catch_up_limit: Option<i32>,
    pub(crate) retry_delay_secs: Option<i64>,
    pub(crate) max_failures: i32,
}

pub(crate) struct ScheduleCreateOptions {
    pub(crate) definition: ScheduleDefinitionOptions,
    pub(crate) clients: Vec<String>,
    pub(crate) tags: Vec<String>,
    pub(crate) confirmed: bool,
}

pub(crate) struct ScheduleUpdateOptions {
    pub(crate) schedule_id: String,
    pub(crate) definition: ScheduleDefinitionOptions,
    pub(crate) clients: Vec<String>,
    pub(crate) tags: Vec<String>,
    pub(crate) confirmed: bool,
}

#[derive(Debug)]
struct ScheduleDefinition {
    name: String,
    trigger_kind: ScheduleTriggerKindArg,
    operation: Option<JobCommand>,
    event_argv_template: Option<Vec<String>>,
    cron_expr: Option<String>,
    timezone: Option<String>,
    event_expression: Option<String>,
    enabled: bool,
    catch_up_policy: Option<String>,
    catch_up_limit: Option<i32>,
    retry_delay_secs: Option<i64>,
    max_failures: i32,
}

impl ScheduleDefinition {
    fn from_options(options: ScheduleDefinitionOptions) -> Result<Self> {
        anyhow::ensure!(
            !options.name.trim().is_empty(),
            "schedule name must not be empty"
        );
        anyhow::ensure!(
            (1..=100).contains(&options.max_failures),
            "--max-failures must be between 1 and 100"
        );
        match options.trigger_kind {
            ScheduleTriggerKindArg::Cron => {
                anyhow::ensure!(
                    options.event_expression.is_none() && options.event_argv_template.is_empty(),
                    "cron schedules do not accept --event-expression or --event-argv-template"
                );
                let command = options
                    .command
                    .context("cron schedules require --command")?;
                anyhow::ensure!(!command.trim().is_empty(), "--command must not be empty");
                let operation = JobCommand::Shell {
                    argv: if options.argv.is_empty() {
                        vec![command]
                    } else {
                        options.argv
                    },
                    pty: options.pty,
                };
                let cron_expr = options.cron_expr.unwrap_or_else(|| "0 * * * *".to_string());
                anyhow::ensure!(
                    cron_expr.split_whitespace().count() == 5,
                    "--cron-expr must contain exactly five fields"
                );
                let timezone = options.timezone.unwrap_or_else(|| "UTC".to_string());
                anyhow::ensure!(timezone == "UTC", "--timezone must be UTC");
                let catch_up_policy = options
                    .catch_up_policy
                    .unwrap_or_else(|| "skip_missed".to_string());
                let catch_up_limit = options.catch_up_limit.unwrap_or(1);
                let retry_delay_secs = options.retry_delay_secs.unwrap_or(300);
                validate_schedule_policy(
                    &catch_up_policy,
                    catch_up_limit,
                    retry_delay_secs,
                    options.max_failures,
                )?;
                Ok(Self {
                    name: options.name,
                    trigger_kind: options.trigger_kind,
                    operation: Some(operation),
                    event_argv_template: None,
                    cron_expr: Some(cron_expr),
                    timezone: Some(timezone),
                    event_expression: None,
                    enabled: !options.disabled,
                    catch_up_policy: Some(catch_up_policy),
                    catch_up_limit: Some(catch_up_limit),
                    retry_delay_secs: Some(retry_delay_secs),
                    max_failures: options.max_failures,
                })
            }
            ScheduleTriggerKindArg::Event => {
                anyhow::ensure!(
                    options.command.is_none() && options.argv.is_empty() && !options.pty,
                    "alert-event schedules use --event-argv-template; omit --command, --argv, and --pty"
                );
                anyhow::ensure!(
                    options.cron_expr.is_none()
                        && options.timezone.is_none()
                        && options.catch_up_policy.is_none()
                        && options.catch_up_limit.is_none()
                        && options.retry_delay_secs.is_none(),
                    "alert-event schedules do not accept cron, timezone, catch-up, or retry options"
                );
                let event_expression = options
                    .event_expression
                    .context("alert-event schedules require --event-expression")?;
                parse_and_validate_alert_event_expression(&event_expression)
                    .map_err(anyhow::Error::msg)
                    .context(
                        "invalid --event-expression; use the Schedule web UI for per-edge server preview",
                    )?;
                let event_argv_template = if options.event_argv_template.is_empty() {
                    None
                } else {
                    Some(options.event_argv_template)
                };
                validate_alert_event_argv_template(event_argv_template.as_deref())
                    .map_err(anyhow::Error::msg)
                    .context(
                        "invalid --event-argv-template; use the Schedule web UI for per-edge server preview",
                    )?;
                Ok(Self {
                    name: options.name,
                    trigger_kind: options.trigger_kind,
                    operation: None,
                    event_argv_template,
                    cron_expr: None,
                    timezone: None,
                    event_expression: Some(event_expression.trim().to_string()),
                    enabled: !options.disabled,
                    catch_up_policy: None,
                    catch_up_limit: None,
                    retry_delay_secs: None,
                    max_failures: options.max_failures,
                })
            }
        }
    }

    fn command_type(&self) -> &'static str {
        match self.trigger_kind {
            ScheduleTriggerKindArg::Event => "shell",
            ScheduleTriggerKindArg::Cron
                if matches!(
                    self.operation.as_ref(),
                    Some(JobCommand::Shell { pty: true, .. })
                ) =>
            {
                "shell_pty"
            }
            ScheduleTriggerKindArg::Cron => "shell_argv",
        }
    }

    fn privilege_payload(&self) -> Result<SchedulePrivilegePayload<'_>> {
        match self.trigger_kind {
            ScheduleTriggerKindArg::Cron => self
                .operation
                .as_ref()
                .map(SchedulePrivilegePayload::Operation)
                .context("validated cron schedule is missing its operation"),
            ScheduleTriggerKindArg::Event => Ok(SchedulePrivilegePayload::AlertEventArgv(
                self.event_argv_template.as_deref(),
            )),
        }
    }
}

pub(crate) fn schedules(api_url: &str, token: Option<&str>) -> Result<()> {
    println!("{}", http_get(api_url, "/api/v1/schedules", token)?);
    Ok(())
}

pub(crate) fn schedule_create(
    api_url: &str,
    token: Option<&str>,
    options: ScheduleCreateOptions,
) -> Result<()> {
    anyhow::ensure!(options.confirmed, "schedule-create requires --confirmed");
    let definition = ScheduleDefinition::from_options(options.definition)?;
    let selector_expression = selector_expression_from_targets(&options.clients, &options.tags);
    anyhow::ensure!(
        !selector_expression.is_empty(),
        "schedule-create requires at least one target selector"
    );
    let target_ids = resolve_schedule_target_ids(api_url, token, &selector_expression)?;
    let privilege_assertion = schedule_privilege_assertion(SchedulePrivilegeRequest {
        action: "schedule.create",
        schedule_id: None,
        definition_revision: None,
        name: &definition.name,
        payload: definition.privilege_payload()?,
        command_type: definition.command_type(),
        selector_expression: &selector_expression,
        resolved_targets: &target_ids,
        trigger_kind: definition.trigger_kind.as_str(),
        cron_expr: definition.cron_expr.as_deref(),
        timezone: definition.timezone.as_deref(),
        event_expression: definition.event_expression.as_deref(),
        enabled: definition.enabled,
        catch_up_policy: definition.catch_up_policy.as_deref(),
        catch_up_limit: definition.catch_up_limit,
        retry_delay_secs: definition.retry_delay_secs,
        max_failures: definition.max_failures,
        deferred_until: None,
        deleted: false,
    })?;
    println!(
        "{}",
        http_post_json(
            api_url,
            "/api/v1/schedules",
            token,
            &serde_json::json!({
                "name": definition.name,
                "operation": definition.operation,
                "event_argv_template": definition.event_argv_template,
                "selector_expression": selector_expression,
                "target_client_ids": target_ids,
                "trigger_kind": definition.trigger_kind,
                "cron_expr": definition.cron_expr,
                "timezone": definition.timezone,
                "event_expression": definition.event_expression,
                "enabled": definition.enabled,
                "catch_up_policy": definition.catch_up_policy,
                "catch_up_limit": definition.catch_up_limit,
                "retry_delay_secs": definition.retry_delay_secs,
                "max_failures": definition.max_failures,
                "confirmed": options.confirmed,
                "privilege_assertion": privilege_assertion,
            }),
        )?
    );
    Ok(())
}

pub(crate) fn schedule_update(
    api_url: &str,
    token: Option<&str>,
    options: ScheduleUpdateOptions,
) -> Result<()> {
    anyhow::ensure!(options.confirmed, "schedule-update requires --confirmed");
    let definition = ScheduleDefinition::from_options(options.definition)?;
    let selector_expression = selector_expression_from_targets(&options.clients, &options.tags);
    anyhow::ensure!(
        !selector_expression.is_empty(),
        "schedule-update requires at least one target selector"
    );
    let current = schedule_by_id(api_url, token, &options.schedule_id)?;
    let target_ids = if current.selector_expression.trim() == selector_expression.trim() {
        current.target_client_ids.clone()
    } else {
        resolve_schedule_target_ids(api_url, token, &selector_expression)?
    };
    let privilege_assertion = schedule_privilege_assertion(SchedulePrivilegeRequest {
        action: "schedule.update",
        schedule_id: Some(&current.id),
        definition_revision: Some(current.definition_revision),
        name: &definition.name,
        payload: definition.privilege_payload()?,
        command_type: definition.command_type(),
        selector_expression: &selector_expression,
        resolved_targets: &target_ids,
        trigger_kind: definition.trigger_kind.as_str(),
        cron_expr: definition.cron_expr.as_deref(),
        timezone: definition.timezone.as_deref(),
        event_expression: definition.event_expression.as_deref(),
        enabled: definition.enabled,
        catch_up_policy: definition.catch_up_policy.as_deref(),
        catch_up_limit: definition.catch_up_limit,
        retry_delay_secs: definition.retry_delay_secs,
        max_failures: definition.max_failures,
        deferred_until: None,
        deleted: false,
    })?;
    println!(
        "{}",
        http_put_json(
            api_url,
            &format!("/api/v1/schedules/{}", options.schedule_id),
            token,
            &serde_json::json!({
                "name": definition.name,
                "operation": definition.operation,
                "event_argv_template": definition.event_argv_template,
                "selector_expression": selector_expression,
                "target_client_ids": target_ids,
                "expected_selector_expression": current.selector_expression,
                "expected_target_client_ids": current.target_client_ids,
                "expected_definition_revision": current.definition_revision,
                "trigger_kind": definition.trigger_kind,
                "cron_expr": definition.cron_expr,
                "timezone": definition.timezone,
                "event_expression": definition.event_expression,
                "enabled": definition.enabled,
                "catch_up_policy": definition.catch_up_policy,
                "catch_up_limit": definition.catch_up_limit,
                "retry_delay_secs": definition.retry_delay_secs,
                "max_failures": definition.max_failures,
                "confirmed": options.confirmed,
                "privilege_assertion": privilege_assertion,
            }),
        )?
    );
    Ok(())
}

pub(crate) fn schedule_enable(
    api_url: &str,
    token: Option<&str>,
    schedule_id: String,
    confirmed: bool,
) -> Result<()> {
    schedule_state_mutation(
        api_url,
        token,
        &schedule_id,
        "schedule.enable",
        "enable",
        true,
        confirmed,
    )
}

pub(crate) fn schedule_disable(
    api_url: &str,
    token: Option<&str>,
    schedule_id: String,
    confirmed: bool,
) -> Result<()> {
    schedule_state_mutation(
        api_url,
        token,
        &schedule_id,
        "schedule.disable",
        "disable",
        false,
        confirmed,
    )
}

pub(crate) fn schedule_defer(
    api_url: &str,
    token: Option<&str>,
    schedule_id: String,
    deferred_until: String,
    reason: Option<String>,
    confirmed: bool,
) -> Result<()> {
    anyhow::ensure!(confirmed, "schedule-defer requires --confirmed");
    let schedule = schedule_by_id(api_url, token, &schedule_id)?;
    let privilege_assertion = schedule_privilege_for_record(
        "schedule.defer",
        &schedule,
        &schedule.target_client_ids,
        schedule.enabled,
        Some(&deferred_until),
        false,
    )?;
    println!(
        "{}",
        http_post_json(
            api_url,
            &format!("/api/v1/schedules/{schedule_id}/defer"),
            token,
            &serde_json::json!({
                "deferred_until": deferred_until,
                "expected_definition_revision": schedule.definition_revision,
                "reason": reason,
                "confirmed": confirmed,
                "privilege_assertion": privilege_assertion,
            }),
        )?
    );
    Ok(())
}

pub(crate) fn schedule_apply_now(
    api_url: &str,
    token: Option<&str>,
    schedule_id: String,
    confirmed: bool,
) -> Result<()> {
    anyhow::ensure!(confirmed, "schedule-apply-now requires --confirmed");
    let schedule = schedule_by_id(api_url, token, &schedule_id)?;
    validate_apply_now_trigger(schedule.trigger_kind)?;
    let privilege_assertion = schedule_privilege_for_record(
        "schedule.apply_now",
        &schedule,
        &schedule.target_client_ids,
        schedule.enabled,
        schedule.deferred_until.as_deref(),
        false,
    )?;
    println!(
        "{}",
        http_post_json(
            api_url,
            &format!("/api/v1/schedules/{schedule_id}/apply-now"),
            token,
            &serde_json::json!({
                "expected_definition_revision": schedule.definition_revision,
                "confirmed": confirmed,
                "privilege_assertion": privilege_assertion,
            }),
        )?
    );
    Ok(())
}

fn validate_apply_now_trigger(trigger_kind: ScheduleTriggerKindArg) -> Result<()> {
    anyhow::ensure!(
        trigger_kind == ScheduleTriggerKindArg::Cron,
        "schedule-apply-now is only available for cron schedules; alert-event schedules dispatch only on matching alert edges"
    );
    Ok(())
}

pub(crate) fn schedule_refresh_targets(
    api_url: &str,
    token: Option<&str>,
    schedule_id: String,
    confirmed: bool,
) -> Result<()> {
    anyhow::ensure!(confirmed, "schedule-refresh-targets requires --confirmed");
    let schedule = schedule_by_id(api_url, token, &schedule_id)?;
    anyhow::ensure!(
        !schedule.selector_expression.trim().is_empty(),
        "schedule has no selector expression to refresh"
    );
    let target_ids =
        resolve_schedule_target_ids_allow_empty(api_url, token, &schedule.selector_expression)?;
    anyhow::ensure!(
        normalized_ids(&target_ids) != normalized_ids(&schedule.target_client_ids),
        "schedule targets are already current"
    );
    let privilege_assertion = schedule_privilege_for_record(
        "schedule.targets.update",
        &schedule,
        &target_ids,
        schedule.enabled,
        schedule.deferred_until.as_deref(),
        false,
    )?;
    println!(
        "{}",
        http_post_json(
            api_url,
            &format!("/api/v1/schedules/{schedule_id}/targets"),
            token,
            &serde_json::json!({
                "expected_definition_revision": schedule.definition_revision,
                "confirmed": confirmed,
                "privilege_assertion": privilege_assertion,
            }),
        )?
    );
    Ok(())
}

pub(crate) fn schedule_delete(
    api_url: &str,
    token: Option<&str>,
    schedule_id: String,
    confirmed: bool,
) -> Result<()> {
    anyhow::ensure!(confirmed, "schedule-delete requires --confirmed");
    let schedule = schedule_by_id(api_url, token, &schedule_id)?;
    let privilege_assertion = schedule_privilege_for_record(
        "schedule.delete",
        &schedule,
        &schedule.target_client_ids,
        false,
        schedule.deferred_until.as_deref(),
        true,
    )?;
    println!(
        "{}",
        http_delete_json(
            api_url,
            &format!("/api/v1/schedules/{schedule_id}"),
            token,
            &serde_json::json!({
                "expected_definition_revision": schedule.definition_revision,
                "confirmed": confirmed,
                "privilege_assertion": privilege_assertion,
            }),
        )?
    );
    Ok(())
}

fn schedule_state_mutation(
    api_url: &str,
    token: Option<&str>,
    schedule_id: &str,
    action: &str,
    endpoint: &str,
    enabled: bool,
    confirmed: bool,
) -> Result<()> {
    anyhow::ensure!(confirmed, "schedule-{endpoint} requires --confirmed");
    let schedule = schedule_by_id(api_url, token, schedule_id)?;
    let privilege_assertion = schedule_privilege_for_record(
        action,
        &schedule,
        &schedule.target_client_ids,
        enabled,
        schedule.deferred_until.as_deref(),
        false,
    )?;
    println!(
        "{}",
        http_post_json(
            api_url,
            &format!("/api/v1/schedules/{schedule_id}/{endpoint}"),
            token,
            &serde_json::json!({
                "expected_definition_revision": schedule.definition_revision,
                "confirmed": confirmed,
                "privilege_assertion": privilege_assertion,
            }),
        )?
    );
    Ok(())
}

fn schedule_by_id(api_url: &str, token: Option<&str>, schedule_id: &str) -> Result<ScheduleRecord> {
    let schedule_id = uuid::Uuid::parse_str(schedule_id).context("invalid schedule UUID")?;
    let body = http_get(api_url, &format!("/api/v1/schedules/{schedule_id}"), token)?;
    serde_json::from_str(&body).context("failed to parse schedule")
}

pub(crate) fn saved_schedule_target_snapshot(
    api_url: &str,
    token: Option<&str>,
    schedule_id: &str,
) -> Result<SavedScheduleTargetSnapshot> {
    let schedule = schedule_by_id(api_url, token, schedule_id)?;
    validate_backup_schedule_trigger(schedule.trigger_kind)?;
    Ok(SavedScheduleTargetSnapshot {
        selector_expression: schedule.selector_expression,
        target_client_ids: schedule.target_client_ids,
        definition_revision: schedule.definition_revision,
    })
}

fn validate_backup_schedule_trigger(trigger_kind: ScheduleTriggerKindArg) -> Result<()> {
    anyhow::ensure!(
        trigger_kind == ScheduleTriggerKindArg::Cron,
        "backup policies can only update cron schedules"
    );
    Ok(())
}

fn schedule_privilege_for_record(
    action: &str,
    schedule: &ScheduleRecord,
    resolved_targets: &[String],
    enabled: bool,
    deferred_until: Option<&str>,
    deleted: bool,
) -> Result<vpsman_common::PrivilegeAssertion> {
    schedule_privilege_assertion(SchedulePrivilegeRequest {
        action,
        schedule_id: Some(&schedule.id),
        definition_revision: Some(schedule.definition_revision),
        name: &schedule.name,
        payload: SchedulePrivilegePayload::StoredHash(&schedule.operation_payload_hash),
        command_type: &schedule.command_type,
        selector_expression: &schedule.selector_expression,
        resolved_targets,
        trigger_kind: schedule.trigger_kind.as_str(),
        cron_expr: schedule.cron_expr.as_deref(),
        timezone: schedule.timezone.as_deref(),
        event_expression: schedule.event_expression.as_deref(),
        enabled,
        catch_up_policy: schedule.catch_up_policy.as_deref(),
        catch_up_limit: schedule.catch_up_limit,
        retry_delay_secs: schedule.retry_delay_secs,
        max_failures: schedule.max_failures,
        deferred_until,
        deleted,
    })
}

fn schedule_privilege_assertion(
    request: SchedulePrivilegeRequest<'_>,
) -> Result<vpsman_common::PrivilegeAssertion> {
    let password = load_super_password("VPSMAN_SUPER_PASSWORD")?;
    let salt_hex = load_super_salt_hex(None)?;
    build_privilege_for_schedule(request, &password, &salt_hex, 300)
}

pub(crate) fn selector_expression_from_targets(clients: &[String], tags: &[String]) -> String {
    clients
        .iter()
        .map(|client_id| format!("id:{client_id}"))
        .chain(tags.iter().map(|tag| selector_token_from_tag_argument(tag)))
        .collect::<Vec<_>>()
        .join(" || ")
}

fn selector_token_from_tag_argument(value: &str) -> String {
    if matches!(
        value.split_once(':').map(|(namespace, _)| namespace),
        Some("id" | "name" | "tag" | "provider" | "country" | "region" | "status")
    ) {
        value.to_string()
    } else {
        format!("tag:{value}")
    }
}

pub(crate) fn resolve_schedule_target_ids(
    api_url: &str,
    token: Option<&str>,
    selector_expression: &str,
) -> Result<Vec<String>> {
    let target_ids = resolve_schedule_target_ids_allow_empty(api_url, token, selector_expression)?;
    anyhow::ensure!(
        !target_ids.is_empty(),
        "schedule resolved no targets; provide at least one matching target"
    );
    Ok(target_ids)
}

fn resolve_schedule_target_ids_allow_empty(
    api_url: &str,
    token: Option<&str>,
    selector_expression: &str,
) -> Result<Vec<String>> {
    #[derive(Deserialize)]
    struct BulkResolveResponse {
        targets: Vec<BulkTarget>,
    }
    #[derive(Deserialize)]
    struct BulkTarget {
        id: String,
    }
    let body = http_post_json(
        api_url,
        "/api/v1/bulk/resolve",
        token,
        &serde_json::json!({
            "selector_expression": selector_expression,
        }),
    )?;
    let response: BulkResolveResponse =
        serde_json::from_str(&body).context("failed to parse bulk target response")?;
    Ok(normalized_ids(
        &response
            .targets
            .into_iter()
            .map(|target| target.id)
            .collect::<Vec<_>>(),
    ))
}

fn normalized_ids(values: &[String]) -> Vec<String> {
    let mut values = values.to_vec();
    values.sort();
    values.dedup();
    values
}

fn validate_schedule_policy(
    catch_up_policy: &str,
    catch_up_limit: i32,
    retry_delay_secs: i64,
    max_failures: i32,
) -> Result<()> {
    anyhow::ensure!(
        matches!(
            catch_up_policy,
            "skip_missed" | "run_once" | "run_all_limited"
        ),
        "--catch-up-policy must be skip_missed, run_once, or run_all_limited"
    );
    anyhow::ensure!(
        (1..=25).contains(&catch_up_limit),
        "--catch-up-limit must be between 1 and 25"
    );
    anyhow::ensure!(
        (1..=86_400).contains(&retry_delay_secs),
        "--retry-delay-secs must be between 1 and 86400"
    );
    anyhow::ensure!(
        (1..=100).contains(&max_failures),
        "--max-failures must be between 1 and 100"
    );
    Ok(())
}

#[cfg(test)]
#[path = "tests_commands_schedules.rs"]
mod tests;
