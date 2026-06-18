use anyhow::{ensure, Context, Result};
use vpsman_common::{
    canonical_db_privilege_intent, canonical_job_privilege_intent,
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
    pub(crate) name: &'a str,
    pub(crate) command: &'a JobCommand,
    pub(crate) command_type: &'a str,
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

pub(crate) fn build_privilege_for_job_command(
    client_ids: &[String],
    command: &JobCommand,
    command_type: &str,
    selector_expression: &str,
    password: &str,
    salt_hex: &str,
    ttl_secs: u64,
    timeout_secs: u64,
    force_unprivileged: bool,
    privileged: bool,
) -> Result<BuiltJobPrivilege> {
    let payload_hash_hex = payload_hash(&encode_json(command)?);
    build_privilege_for_payload_hash(
        client_ids,
        &payload_hash_hex,
        command_type,
        selector_expression,
        password,
        salt_hex,
        ttl_secs,
        timeout_secs,
        force_unprivileged,
        privileged,
    )
}

pub(crate) fn build_privilege_for_payload_hash(
    client_ids: &[String],
    payload_hash_hex: &str,
    command_type: &str,
    selector_expression: &str,
    password: &str,
    salt_hex: &str,
    ttl_secs: u64,
    timeout_secs: u64,
    force_unprivileged: bool,
    privileged: bool,
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
        resolved_targets: client_ids,
        timeout_secs,
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
    let payload_hash_hex = payload_hash(&encode_json(request.command)?);
    let intent = canonical_schedule_privilege_intent(SchedulePrivilegeIntentInput {
        action: request.action,
        schedule_id: request.schedule_id,
        name: request.name,
        command_type: request.command_type,
        operation_payload_hash: &payload_hash_hex,
        selector_expression: request.selector_expression,
        resolved_targets: request.resolved_targets,
        cron_expr: request.cron_expr,
        timezone: request.timezone,
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
        None,
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
mod tests {
    use super::*;
    use vpsman_common::{verify_privilege_assertion, PrivilegeAssertionReplayCache};

    #[test]
    fn builds_job_privilege_assertion_without_command_envelopes() {
        let clients = vec!["client-b".to_string(), "client-a".to_string()];
        let command = JobCommand::Shell {
            argv: vec!["/bin/true".to_string()],
            pty: false,
        };
        let built = build_privilege_for_job_command(
            &clients,
            &command,
            "shell_argv",
            "id:client-a || id:client-b",
            "correct horse",
            "01020304",
            300,
            30,
            false,
            true,
        )
        .unwrap();

        let payload_hash_hex = payload_hash(&encode_json(&command).unwrap());
        assert_eq!(
            built.privilege_assertion.expires_unix,
            built.privilege_assertion.issued_unix + 300
        );

        let verifier_key = derive_super_key("correct horse", &[1, 2, 3, 4]);
        let intent = canonical_job_privilege_intent(JobPrivilegeIntentInput {
            selector_expression: "id:client-a || id:client-b",
            command_type: "shell_argv",
            operation_payload_hash: &payload_hash_hex,
            resolved_targets: &clients,
            timeout_secs: 30,
            force_unprivileged: false,
            privileged: true,
        })
        .unwrap();
        assert!(verify_privilege_assertion(
            &verifier_key,
            &intent,
            &built.privilege_assertion,
            built.privilege_assertion.issued_unix,
            &mut PrivilegeAssertionReplayCache::default(),
        )
        .is_ok());
    }

    #[test]
    fn builds_schedule_privilege_assertion_for_resolved_targets() {
        let clients = vec!["client-a".to_string()];
        let command = JobCommand::Shell {
            argv: vec!["/bin/true".to_string()],
            pty: false,
        };
        let assertion = build_privilege_for_schedule(
            SchedulePrivilegeRequest {
                action: "schedule.create",
                schedule_id: None,
                name: "nightly",
                command: &command,
                command_type: "shell_argv",
                selector_expression: "id:client-a",
                resolved_targets: &clients,
                cron_expr: "0 3 * * *",
                timezone: "UTC",
                enabled: true,
                catch_up_policy: "skip_missed",
                catch_up_limit: 1,
                retry_delay_secs: 60,
                max_failures: 3,
                deferred_until: None,
                deleted: false,
            },
            "correct horse",
            "01020304",
            120,
        )
        .unwrap();

        assert_eq!(assertion.expires_unix, assertion.issued_unix + 120);
        assert_eq!(assertion.nonce_hex.len(), 32);
        assert_eq!(assertion.assertion_hex.len(), 64);
    }
}
