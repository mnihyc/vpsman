use anyhow::{ensure, Context, Result};
use vpsman_common::{
    alert_event_argv_template_hash, canonical_db_privilege_intent, canonical_job_privilege_intent,
    canonical_schedule_privilege_intent, derive_super_key, encode_json, payload_hash, random_nonce,
    sign_privilege_assertion, JobCommand, JobPrivilegeIntentInput, PrivilegeAssertion,
    SchedulePrivilegeIntentInput,
};

use crate::unix_now;

#[derive(Clone, Debug)]
pub(crate) struct BuiltJobPrivilege {
    pub(crate) privilege_assertion: PrivilegeAssertion,
}

pub(crate) struct SchedulePrivilegeRequest<'a> {
    pub(crate) action: &'a str,
    pub(crate) schedule_id: Option<&'a str>,
    pub(crate) definition_revision: Option<i64>,
    pub(crate) name: &'a str,
    pub(crate) payload: SchedulePrivilegePayload<'a>,
    pub(crate) command_type: &'a str,
    pub(crate) selector_expression: &'a str,
    pub(crate) resolved_targets: &'a [String],
    pub(crate) trigger_kind: &'a str,
    pub(crate) cron_expr: Option<&'a str>,
    pub(crate) timezone: Option<&'a str>,
    pub(crate) event_expression: Option<&'a str>,
    pub(crate) enabled: bool,
    pub(crate) catch_up_policy: Option<&'a str>,
    pub(crate) catch_up_limit: Option<i32>,
    pub(crate) retry_delay_secs: Option<i64>,
    pub(crate) max_failures: i32,
    pub(crate) deferred_until: Option<&'a str>,
    pub(crate) deleted: bool,
}

pub(crate) enum SchedulePrivilegePayload<'a> {
    Operation(&'a JobCommand),
    AlertEventArgv(Option<&'a [String]>),
    StoredHash(&'a str),
}

pub(crate) fn build_privilege_for_job_command(
    client_ids: &[String],
    command: &JobCommand,
    command_type: &str,
    selector_expression: &str,
    password: &str,
    salt_hex: &str,
    ttl_secs: u64,
    max_timeout_secs: u64,
    force_unprivileged: bool,
    privileged: bool,
) -> Result<BuiltJobPrivilege> {
    build_privilege_for_job_command_with_rollout_hash(
        client_ids,
        command,
        command_type,
        selector_expression,
        password,
        salt_hex,
        ttl_secs,
        max_timeout_secs,
        force_unprivileged,
        privileged,
        None,
    )
}

pub(crate) fn build_privilege_for_job_command_with_rollout_hash(
    client_ids: &[String],
    command: &JobCommand,
    command_type: &str,
    selector_expression: &str,
    password: &str,
    salt_hex: &str,
    ttl_secs: u64,
    max_timeout_secs: u64,
    force_unprivileged: bool,
    privileged: bool,
    rollout_policy_hash: Option<&str>,
) -> Result<BuiltJobPrivilege> {
    let payload_hash_hex = payload_hash(&encode_json(command)?);
    build_privilege_for_payload_hash_with_rollout_hash(
        client_ids,
        &payload_hash_hex,
        command_type,
        selector_expression,
        password,
        salt_hex,
        ttl_secs,
        max_timeout_secs,
        force_unprivileged,
        privileged,
        rollout_policy_hash,
    )
}

pub(crate) fn build_privilege_for_payload_hash_with_rollout_hash(
    client_ids: &[String],
    payload_hash_hex: &str,
    command_type: &str,
    selector_expression: &str,
    password: &str,
    salt_hex: &str,
    ttl_secs: u64,
    max_timeout_secs: u64,
    force_unprivileged: bool,
    privileged: bool,
    rollout_policy_hash: Option<&str>,
) -> Result<BuiltJobPrivilege> {
    anyhow::ensure!(
        !client_ids.is_empty(),
        "privilege unlock resolved no clients"
    );
    let payload_hash_hex = normalize_sha256_hex(payload_hash_hex)?;
    let intent = canonical_job_privilege_intent(JobPrivilegeIntentInput {
        selector_expression,
        command_type,
        operation_payload_hash: &payload_hash_hex,
        rollout_policy_hash,
        resolved_targets: client_ids,
        max_timeout_secs,
        force_unprivileged,
        privileged,
    })?;
    let assertion = build_privilege_assertion(&intent, password, salt_hex, ttl_secs)?;
    Ok(BuiltJobPrivilege {
        privilege_assertion: assertion,
    })
}

pub(crate) fn build_privilege_assertion(
    intent: &str,
    password: &str,
    salt_hex: &str,
    ttl_secs: u64,
) -> Result<PrivilegeAssertion> {
    ensure!(
        (15..=300).contains(&ttl_secs),
        "privilege TTL must be between 15 and 300 seconds"
    );
    let salt = decode_super_salt(salt_hex)?;
    let verifier_key = derive_super_key(password, &salt);
    let intent_hash_hex = payload_hash(intent.as_bytes());
    let issued_unix = unix_now();
    let expires_unix = issued_unix.saturating_add(ttl_secs);
    Ok(sign_privilege_assertion(
        &verifier_key,
        &intent_hash_hex,
        &random_nonce(),
        issued_unix,
        expires_unix,
    ))
}

pub(crate) fn build_privilege_for_schedule(
    request: SchedulePrivilegeRequest<'_>,
    password: &str,
    salt_hex: &str,
    ttl_secs: u64,
) -> Result<PrivilegeAssertion> {
    let payload_hash_hex = match request.payload {
        SchedulePrivilegePayload::Operation(command) => payload_hash(&encode_json(command)?),
        SchedulePrivilegePayload::AlertEventArgv(template) => {
            alert_event_argv_template_hash(template).map_err(anyhow::Error::msg)?
        }
        SchedulePrivilegePayload::StoredHash(value) => normalize_sha256_hex(value)?,
    };
    let intent = canonical_schedule_privilege_intent(SchedulePrivilegeIntentInput {
        action: request.action,
        schedule_id: request.schedule_id,
        definition_revision: request.definition_revision,
        name: request.name,
        command_type: request.command_type,
        operation_payload_hash: &payload_hash_hex,
        selector_expression: request.selector_expression,
        resolved_targets: request.resolved_targets,
        trigger_kind: request.trigger_kind,
        cron_expr: request.cron_expr,
        timezone: request.timezone,
        event_expression: request.event_expression,
        enabled: request.enabled,
        catch_up_policy: request.catch_up_policy,
        catch_up_limit: request.catch_up_limit,
        retry_delay_secs: request.retry_delay_secs,
        max_failures: request.max_failures,
        deferred_until: request.deferred_until,
        deleted: request.deleted,
    })?;
    build_privilege_assertion(&intent, password, salt_hex, ttl_secs)
}

pub(crate) struct DbPrivilegeRequest<'a> {
    pub(crate) action: &'a str,
    pub(crate) target: &'a str,
    pub(crate) selector_expression: Option<&'a str>,
    pub(crate) resolved_targets: &'a [String],
    pub(crate) confirmed: bool,
    pub(crate) payload_hash: Option<&'a str>,
}

pub(crate) fn build_privilege_for_db(
    request: DbPrivilegeRequest<'_>,
    password: &str,
    salt_hex: &str,
    ttl_secs: u64,
) -> Result<PrivilegeAssertion> {
    let intent = canonical_db_privilege_intent(
        request.action,
        request.target,
        request.selector_expression,
        request.resolved_targets,
        request.confirmed,
        request.payload_hash,
    )?;
    build_privilege_assertion(&intent, password, salt_hex, ttl_secs)
}

fn normalize_sha256_hex(value: &str) -> Result<String> {
    let normalized = value.trim().to_ascii_lowercase();
    anyhow::ensure!(
        normalized.len() == 64
            && normalized
                .chars()
                .all(|character| character.is_ascii_hexdigit()),
        "payload hash must be 32-byte hex"
    );
    Ok(normalized)
}

pub(crate) fn decode_super_salt(salt_hex: &str) -> Result<Vec<u8>> {
    let salt = hex::decode(salt_hex.trim()).context("super-password salt is not valid hex")?;
    anyhow::ensure!(
        !salt.is_empty(),
        "super-password salt decodes to empty salt"
    );
    Ok(salt)
}

pub(crate) fn load_super_password(password_env: &str) -> Result<String> {
    let password = std::env::var(password_env)
        .with_context(|| format!("environment variable {password_env} is not set"))?;
    anyhow::ensure!(
        !password.is_empty(),
        "environment variable {password_env} is empty"
    );
    Ok(password)
}

pub(crate) fn load_super_salt_hex(explicit_salt_hex: Option<&str>) -> Result<String> {
    let salt_hex = match explicit_salt_hex {
        Some(value) => value.to_string(),
        None => std::env::var("VPSMAN_SUPER_SALT_HEX")
            .context("set --super-salt-hex or VPSMAN_SUPER_SALT_HEX for local privilege unlock")?,
    };
    decode_super_salt(&salt_hex)?;
    Ok(salt_hex)
}

#[cfg(test)]
#[path = "tests_privilege.rs"]
mod tests;
