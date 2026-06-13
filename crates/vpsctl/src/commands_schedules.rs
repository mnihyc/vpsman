use anyhow::{Context, Result};
use serde::Deserialize;
use vpsman_common::JobCommand;

use crate::{
    http::{http_delete_json, http_get, http_post_json, http_put_json},
    privilege::{
        build_privilege_for_schedule, load_super_password, load_super_salt_hex,
        SchedulePrivilegeRequest,
    },
};

#[derive(Debug, Deserialize)]
struct ScheduleRecord {
    id: String,
    name: String,
    enabled: bool,
    command_type: String,
    operation: JobCommand,
    selector_expression: String,
    target_client_ids: Vec<String>,
    cron_expr: String,
    catch_up_policy: String,
    catch_up_limit: i32,
    retry_delay_secs: i64,
    max_failures: i32,
    deferred_until: Option<String>,
}

pub(crate) struct ScheduleCreateOptions {
    pub(crate) name: String,
    pub(crate) command: String,
    pub(crate) argv: Vec<String>,
    pub(crate) pty: bool,
    pub(crate) clients: Vec<String>,
    pub(crate) tags: Vec<String>,
    pub(crate) cron_expr: String,
    pub(crate) disabled: bool,
    pub(crate) catch_up_policy: String,
    pub(crate) catch_up_limit: i32,
    pub(crate) retry_delay_secs: i64,
    pub(crate) max_failures: i32,
}

pub(crate) struct ScheduleUpdateOptions {
    pub(crate) schedule_id: String,
    pub(crate) name: String,
    pub(crate) command: String,
    pub(crate) argv: Vec<String>,
    pub(crate) pty: bool,
    pub(crate) clients: Vec<String>,
    pub(crate) tags: Vec<String>,
    pub(crate) cron_expr: String,
    pub(crate) disabled: bool,
    pub(crate) catch_up_policy: String,
    pub(crate) catch_up_limit: i32,
    pub(crate) retry_delay_secs: i64,
    pub(crate) max_failures: i32,
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
    validate_schedule_policy(
        &options.catch_up_policy,
        options.catch_up_limit,
        options.retry_delay_secs,
        options.max_failures,
    )?;
    let selector_expression = selector_expression_from_targets(&options.clients, &options.tags);
    anyhow::ensure!(
        !selector_expression.is_empty(),
        "schedule-create requires at least one target selector"
    );
    let operation = JobCommand::Shell {
        argv: if options.argv.is_empty() {
            vec![options.command.clone()]
        } else {
            options.argv
        },
        pty: options.pty,
    };
    let password = load_super_password("VPSMAN_SUPER_PASSWORD")?;
    let salt_hex = load_super_salt_hex(None)?;
    let target_ids = resolve_schedule_target_ids(api_url, token, &selector_expression)?;
    let privilege_assertion = build_privilege_for_schedule(
        SchedulePrivilegeRequest {
            action: "schedule.create",
            schedule_id: None,
            name: &options.name,
            command: &operation,
            command_type: if options.pty {
                "shell_pty"
            } else {
                "shell_argv"
            },
            selector_expression: &selector_expression,
            resolved_targets: &target_ids,
            cron_expr: &options.cron_expr,
            timezone: "UTC",
            enabled: !options.disabled,
            catch_up_policy: &options.catch_up_policy,
            catch_up_limit: options.catch_up_limit,
            retry_delay_secs: options.retry_delay_secs,
            max_failures: options.max_failures,
            deferred_until: None,
            deleted: false,
        },
        &password,
        &salt_hex,
        300,
    )?;
    println!(
        "{}",
        http_post_json(
            api_url,
            "/api/v1/schedules",
            token,
            &serde_json::json!({
                "name": options.name,
                "operation": operation,
                "selector_expression": selector_expression,
                "target_client_ids": target_ids,
                "cron_expr": options.cron_expr,
                "timezone": "UTC",
                "enabled": !options.disabled,
                "catch_up_policy": options.catch_up_policy,
                "catch_up_limit": options.catch_up_limit,
                "retry_delay_secs": options.retry_delay_secs,
                "max_failures": options.max_failures,
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
    validate_schedule_policy(
        &options.catch_up_policy,
        options.catch_up_limit,
        options.retry_delay_secs,
        options.max_failures,
    )?;
    let selector_expression = selector_expression_from_targets(&options.clients, &options.tags);
    anyhow::ensure!(
        !selector_expression.is_empty(),
        "schedule-update requires at least one target selector"
    );
    let operation = JobCommand::Shell {
        argv: if options.argv.is_empty() {
            vec![options.command.clone()]
        } else {
            options.argv
        },
        pty: options.pty,
    };
    let target_ids = resolve_schedule_target_ids(api_url, token, &selector_expression)?;
    let privilege_assertion = schedule_privilege_assertion(SchedulePrivilegeRequest {
        action: "schedule.update",
        schedule_id: Some(&options.schedule_id),
        name: &options.name,
        command: &operation,
        command_type: if options.pty {
            "shell_pty"
        } else {
            "shell_argv"
        },
        selector_expression: &selector_expression,
        resolved_targets: &target_ids,
        cron_expr: &options.cron_expr,
        timezone: "UTC",
        enabled: !options.disabled,
        catch_up_policy: &options.catch_up_policy,
        catch_up_limit: options.catch_up_limit,
        retry_delay_secs: options.retry_delay_secs,
        max_failures: options.max_failures,
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
                "name": options.name,
                "operation": operation,
                "selector_expression": selector_expression,
                "target_client_ids": target_ids,
                "cron_expr": options.cron_expr,
                "timezone": "UTC",
                "enabled": !options.disabled,
                "catch_up_policy": options.catch_up_policy,
                "catch_up_limit": options.catch_up_limit,
                "retry_delay_secs": options.retry_delay_secs,
                "max_failures": options.max_failures,
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
) -> Result<()> {
    schedule_state_mutation(
        api_url,
        token,
        &schedule_id,
        "schedule.enable",
        "enable",
        true,
    )
}

pub(crate) fn schedule_disable(
    api_url: &str,
    token: Option<&str>,
    schedule_id: String,
) -> Result<()> {
    schedule_state_mutation(
        api_url,
        token,
        &schedule_id,
        "schedule.disable",
        "disable",
        false,
    )
}

pub(crate) fn schedule_defer(
    api_url: &str,
    token: Option<&str>,
    schedule_id: String,
    deferred_until: String,
    reason: Option<String>,
) -> Result<()> {
    let schedule = schedule_by_id(api_url, token, &schedule_id)?;
    let privilege_assertion = schedule_privilege_for_record(
        api_url,
        token,
        "schedule.defer",
        &schedule,
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
                "reason": reason,
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
) -> Result<()> {
    let schedule = schedule_by_id(api_url, token, &schedule_id)?;
    let privilege_assertion = schedule_privilege_for_record(
        api_url,
        token,
        "schedule.apply_now",
        &schedule,
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
) -> Result<()> {
    let schedule = schedule_by_id(api_url, token, &schedule_id)?;
    let privilege_assertion = schedule_privilege_for_record(
        api_url,
        token,
        "schedule.delete",
        &schedule,
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
) -> Result<()> {
    let schedule = schedule_by_id(api_url, token, schedule_id)?;
    let privilege_assertion = schedule_privilege_for_record(
        api_url,
        token,
        action,
        &schedule,
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
                "privilege_assertion": privilege_assertion,
            }),
        )?
    );
    Ok(())
}

fn schedule_by_id(api_url: &str, token: Option<&str>, schedule_id: &str) -> Result<ScheduleRecord> {
    let body = http_get(api_url, "/api/v1/schedules?limit=1000", token)?;
    let schedules: Vec<ScheduleRecord> =
        serde_json::from_str(&body).context("failed to parse schedule list")?;
    schedules
        .into_iter()
        .find(|schedule| schedule.id == schedule_id)
        .with_context(|| format!("schedule not found: {schedule_id}"))
}

fn schedule_privilege_for_record(
    _api_url: &str,
    _token: Option<&str>,
    action: &str,
    schedule: &ScheduleRecord,
    enabled: bool,
    deferred_until: Option<&str>,
    deleted: bool,
) -> Result<vpsman_common::PrivilegeAssertion> {
    schedule_privilege_assertion(SchedulePrivilegeRequest {
        action,
        schedule_id: Some(&schedule.id),
        name: &schedule.name,
        command: &schedule.operation,
        command_type: &schedule.command_type,
        selector_expression: &schedule.selector_expression,
        resolved_targets: &schedule.target_client_ids,
        cron_expr: &schedule.cron_expr,
        timezone: "UTC",
        enabled,
        catch_up_policy: &schedule.catch_up_policy,
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
    let target_ids = response
        .targets
        .into_iter()
        .map(|target| target.id)
        .collect::<Vec<_>>();
    anyhow::ensure!(
        !target_ids.is_empty(),
        "schedule-create resolved no targets; provide at least one matching target"
    );
    Ok(target_ids)
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
