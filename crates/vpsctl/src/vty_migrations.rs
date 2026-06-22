use anyhow::{Context, Result};
use uuid::Uuid;
use vpsman_common::MAX_CONFIGURABLE_JOB_TIMEOUT_SECS;

use crate::{
    commands_migrations::{migration_link_with_credentials, migration_run_with_credentials},
    vty_jobs::VtyPrivilegeContext,
};

pub(crate) struct VtyMigrationLinkRequest {
    pub(crate) restore_plan_id: Uuid,
    pub(crate) note: Option<String>,
    pub(crate) confirmed: bool,
}

pub(crate) struct VtyMigrationRunRequest {
    pub(crate) restore_plan_id: Uuid,
    pub(crate) archive_transfer_session_id: Uuid,
    pub(crate) note: Option<String>,
    pub(crate) max_timeout_secs: u64,
    pub(crate) confirmed: bool,
    pub(crate) force_unprivileged: bool,
}

pub(crate) fn parse_vty_migration_link(tokens: &[&str]) -> Result<VtyMigrationLinkRequest> {
    let restore_plan_id = tokens
        .first()
        .context("usage: migration-link <restore_plan_uuid> [--note <text>] --confirmed")?;
    let mut request = VtyMigrationLinkRequest {
        restore_plan_id: Uuid::parse_str(restore_plan_id).context("invalid restore plan UUID")?,
        note: None,
        confirmed: false,
    };
    let mut index = 1;
    while index < tokens.len() {
        match tokens[index] {
            "--note" => {
                request.note = Some(
                    tokens
                        .get(index + 1)
                        .context("migration-link --note requires a value")?
                        .to_string(),
                );
                index += 2;
            }
            "--confirmed" => {
                request.confirmed = true;
                index += 1;
            }
            other => anyhow::bail!("unknown migration-link flag {other}"),
        }
    }
    anyhow::ensure!(request.confirmed, "migration-link requires --confirmed");
    Ok(request)
}

pub(crate) fn parse_vty_migration_run(tokens: &[&str]) -> Result<VtyMigrationRunRequest> {
    let restore_plan_id = tokens
        .first()
        .context("usage: migration-run <restore_plan_uuid> --archive-transfer-session-id <uuid> [--note <text>] [--max-timeout <secs>] [--force-unprivileged] --confirmed")?;
    let mut request = VtyMigrationRunRequest {
        restore_plan_id: Uuid::parse_str(restore_plan_id).context("invalid restore plan UUID")?,
        archive_transfer_session_id: Uuid::nil(),
        note: None,
        max_timeout_secs: 60,
        confirmed: false,
        force_unprivileged: false,
    };
    let mut index = 1;
    while index < tokens.len() {
        match tokens[index] {
            "--archive-transfer-session-id" => {
                request.archive_transfer_session_id = Uuid::parse_str(
                    tokens
                        .get(index + 1)
                        .context("migration-run --archive-transfer-session-id requires a value")?,
                )
                .context("invalid migration-run --archive-transfer-session-id")?;
                index += 2;
            }
            value if value.starts_with("--archive-transfer-session-id=") => {
                request.archive_transfer_session_id =
                    Uuid::parse_str(value.trim_start_matches("--archive-transfer-session-id="))
                        .context("invalid migration-run --archive-transfer-session-id")?;
                index += 1;
            }
            "--archive-path" | "--archive-size-bytes" | "--archive-sha256-hex" => {
                anyhow::bail!(
                    "{} was removed; use --archive-transfer-session-id",
                    tokens[index]
                );
            }
            value
                if value.starts_with("--archive-path=")
                    || value.starts_with("--archive-size-bytes=")
                    || value.starts_with("--archive-sha256-hex=") =>
            {
                anyhow::bail!(
                    "archive path/size/SHA flags were removed; use --archive-transfer-session-id"
                );
            }
            "--note" => {
                request.note = Some(
                    tokens
                        .get(index + 1)
                        .context("migration-run --note requires a value")?
                        .to_string(),
                );
                index += 2;
            }
            "--max-timeout" => {
                request.max_timeout_secs = tokens
                    .get(index + 1)
                    .context("migration-run --max-timeout requires a value")?
                    .parse()
                    .context("invalid migration-run --max-timeout")?;
                index += 2;
            }
            "--force-unprivileged" => {
                request.force_unprivileged = true;
                index += 1;
            }
            "--confirmed" => {
                request.confirmed = true;
                index += 1;
            }
            other => anyhow::bail!("unknown migration-run flag {other}"),
        }
    }
    anyhow::ensure!(
        (1..=MAX_CONFIGURABLE_JOB_TIMEOUT_SECS).contains(&request.max_timeout_secs),
        "migration-run timeout out of range"
    );
    anyhow::ensure!(
        !request.archive_transfer_session_id.is_nil(),
        "migration-run requires --archive-transfer-session-id"
    );
    anyhow::ensure!(request.confirmed, "migration-run requires --confirmed");
    Ok(request)
}

pub(crate) fn submit_vty_migration_link(
    api_url: &str,
    token: Option<&str>,
    privilege_context: &VtyPrivilegeContext,
    request: VtyMigrationLinkRequest,
) -> Result<String> {
    migration_link_with_credentials(
        api_url,
        token,
        request.restore_plan_id,
        request.note,
        &privilege_context.password,
        &privilege_context.salt_hex,
        300,
        request.confirmed,
    )
}

pub(crate) fn submit_vty_migration_run(
    api_url: &str,
    token: Option<&str>,
    privilege_context: &VtyPrivilegeContext,
    request: VtyMigrationRunRequest,
) -> Result<String> {
    migration_run_with_credentials(
        api_url,
        token,
        request.restore_plan_id,
        request.archive_transfer_session_id,
        request.note,
        &privilege_context.password,
        &privilege_context.salt_hex,
        300,
        request.max_timeout_secs,
        request.confirmed,
        request.force_unprivileged,
    )
}

#[cfg(test)]
mod tests {
    use super::{parse_vty_migration_link, parse_vty_migration_run};

    #[test]
    fn parses_vty_migration_link() {
        let request = parse_vty_migration_link(&[
            "49c7c3ea-0da8-40b6-b380-5543b1eb3adb",
            "--note",
            "rebuilt",
            "--confirmed",
        ])
        .unwrap();
        assert_eq!(
            request.restore_plan_id.to_string(),
            "49c7c3ea-0da8-40b6-b380-5543b1eb3adb"
        );
        assert_eq!(request.note.as_deref(), Some("rebuilt"));
        assert!(request.confirmed);
    }

    #[test]
    fn rejects_unconfirmed_vty_migration_link() {
        assert!(parse_vty_migration_link(&["49c7c3ea-0da8-40b6-b380-5543b1eb3adb"]).is_err());
    }

    #[test]
    fn parses_vty_migration_run() {
        let archive_transfer_session_id = uuid::Uuid::new_v4();
        let request = parse_vty_migration_run(&[
            "49c7c3ea-0da8-40b6-b380-5543b1eb3adb",
            "--archive-transfer-session-id",
            &archive_transfer_session_id.to_string(),
            "--note",
            "cutover",
            "--max-timeout",
            "120",
            "--force-unprivileged",
            "--confirmed",
        ])
        .unwrap();
        assert_eq!(
            request.restore_plan_id.to_string(),
            "49c7c3ea-0da8-40b6-b380-5543b1eb3adb"
        );
        assert_eq!(
            request.archive_transfer_session_id,
            archive_transfer_session_id
        );
        assert_eq!(request.note.as_deref(), Some("cutover"));
        assert_eq!(request.max_timeout_secs, 120);
        assert!(request.force_unprivileged);
        assert!(request.confirmed);
    }

    #[test]
    fn rejects_unconfirmed_vty_migration_run() {
        assert!(parse_vty_migration_run(&["49c7c3ea-0da8-40b6-b380-5543b1eb3adb"]).is_err());
    }
}
