use serde::Serialize;
use uuid::Uuid;
use vpsman_common::PrivilegeAssertion;

use crate::{state::AppState, ApiError};

#[derive(Serialize)]
pub(crate) struct JobPrivilegeIntent<'a> {
    version: u8,
    action: &'static str,
    selector_expression: &'a str,
    command_type: &'a str,
    operation_payload_hash: &'a str,
    resolved_targets: Vec<&'a str>,
    timeout_secs: u64,
    canary_count: Option<i32>,
    force_unprivileged: bool,
    privileged: bool,
}

impl<'a> JobPrivilegeIntent<'a> {
    pub(crate) fn new(input: JobPrivilegeIntentInput<'a>) -> Self {
        Self {
            version: 1,
            action: "job.dispatch",
            selector_expression: input.selector_expression.trim(),
            command_type: input.command_type,
            operation_payload_hash: input.operation_payload_hash,
            resolved_targets: sorted_refs(input.resolved_targets),
            timeout_secs: input.timeout_secs.clamp(1, 3600),
            canary_count: input.canary_count,
            force_unprivileged: input.force_unprivileged,
            privileged: input.privileged,
        }
    }
}

pub(crate) struct JobPrivilegeIntentInput<'a> {
    pub(crate) selector_expression: &'a str,
    pub(crate) command_type: &'a str,
    pub(crate) operation_payload_hash: &'a str,
    pub(crate) resolved_targets: &'a [String],
    pub(crate) timeout_secs: u64,
    pub(crate) canary_count: Option<i32>,
    pub(crate) force_unprivileged: bool,
    pub(crate) privileged: bool,
}

#[derive(Serialize)]
pub(crate) struct SchedulePrivilegeIntent<'a> {
    version: u8,
    action: &'a str,
    schedule_id: Option<Uuid>,
    name: &'a str,
    command_type: &'a str,
    operation_payload_hash: &'a str,
    selector_expression: &'a str,
    resolved_targets: Vec<&'a str>,
    cron_expr: &'a str,
    timezone: &'a str,
    enabled: bool,
    catch_up_policy: &'a str,
    catch_up_limit: i32,
    retry_delay_secs: i64,
    max_failures: i32,
    deferred_until: Option<&'a str>,
    deleted: bool,
}

impl<'a> SchedulePrivilegeIntent<'a> {
    pub(crate) fn new(input: SchedulePrivilegeIntentInput<'a>) -> Self {
        Self {
            version: 1,
            action: input.action,
            schedule_id: input.schedule_id,
            name: input.name.trim(),
            command_type: input.command_type,
            operation_payload_hash: input.operation_payload_hash,
            selector_expression: input.selector_expression.trim(),
            resolved_targets: sorted_refs(input.resolved_targets),
            cron_expr: input.cron_expr.trim(),
            timezone: input.timezone,
            enabled: input.enabled,
            catch_up_policy: input.catch_up_policy,
            catch_up_limit: input.catch_up_limit,
            retry_delay_secs: input.retry_delay_secs,
            max_failures: input.max_failures,
            deferred_until: input.deferred_until,
            deleted: input.deleted,
        }
    }
}

pub(crate) struct SchedulePrivilegeIntentInput<'a> {
    pub(crate) action: &'a str,
    pub(crate) schedule_id: Option<Uuid>,
    pub(crate) name: &'a str,
    pub(crate) command_type: &'a str,
    pub(crate) operation_payload_hash: &'a str,
    pub(crate) selector_expression: &'a str,
    pub(crate) resolved_targets: &'a [String],
    pub(crate) cron_expr: &'a str,
    pub(crate) timezone: &'a str,
    pub(crate) enabled: bool,
    pub(crate) catch_up_policy: &'a str,
    pub(crate) catch_up_limit: i32,
    pub(crate) retry_delay_secs: i64,
    pub(crate) max_failures: i32,
    pub(crate) deferred_until: Option<&'a str>,
    pub(crate) deleted: bool,
}

#[derive(Serialize)]
pub(crate) struct DbPrivilegeIntent<'a> {
    version: u8,
    action: &'a str,
    target: &'a str,
    selector_expression: Option<&'a str>,
    resolved_targets: Vec<&'a str>,
    confirmed: bool,
}

impl<'a> DbPrivilegeIntent<'a> {
    pub(crate) fn new(
        action: &'a str,
        target: &'a str,
        selector_expression: Option<&'a str>,
        resolved_targets: &'a [String],
        confirmed: bool,
    ) -> Self {
        Self {
            version: 1,
            action,
            target,
            selector_expression: selector_expression.map(str::trim),
            resolved_targets: sorted_refs(resolved_targets),
            confirmed,
        }
    }
}

pub(crate) async fn verify_privilege_intent<T: Serialize>(
    state: &AppState,
    intent: &T,
    assertion: Option<PrivilegeAssertion>,
) -> Result<(), ApiError> {
    if !state.gateway.privilege_configured() {
        return Err(ApiError::conflict("gateway_control_url_missing"));
    }
    #[cfg(test)]
    if state.gateway.test_privilege_auto_approves() {
        return Ok(());
    }
    let assertion = assertion.ok_or_else(|| ApiError::forbidden("privilege_assertion_required"))?;
    let intent = serde_json::to_string(intent)
        .map_err(|error| ApiError::from(anyhow::Error::from(error)))?;
    let result = state
        .gateway
        .verify_privilege(intent, assertion)
        .await
        .map_err(|_| ApiError::forbidden("privilege_verification_failed"))?;
    if result.approved {
        Ok(())
    } else {
        Err(ApiError::forbidden("privilege_verification_denied"))
    }
}

fn sorted_refs(values: &[String]) -> Vec<&str> {
    let mut values = values.iter().map(String::as_str).collect::<Vec<_>>();
    values.sort_unstable();
    values
}
