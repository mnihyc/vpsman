use anyhow::{Context, Result};
use serde_json::json;
use uuid::Uuid;

use crate::{
    commands_migrations::migration_run_with_credentials, http::http_post_json,
    vty_jobs::VtyPrivilegeContext,
};

pub(crate) struct VtyMigrationLinkRequest {
    pub(crate) restore_plan_id: Uuid,
    pub(crate) note: Option<String>,
    pub(crate) confirmed: bool,
}

pub(crate) struct VtyMigrationRunRequest {
    pub(crate) restore_plan_id: Uuid,
    pub(crate) archive_path: String,
    pub(crate) archive_size_bytes: u64,
    pub(crate) archive_sha256_hex: String,
    pub(crate) note: Option<String>,
    pub(crate) timeout_secs: u64,
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
        .context("usage: migration-run <restore_plan_uuid> --archive-path <abs> --archive-size-bytes <bytes> --archive-sha256-hex <sha256> [--note <text>] [--timeout <1-3600>] [--force-unprivileged] --confirmed")?;
    let mut request = VtyMigrationRunRequest {
        restore_plan_id: Uuid::parse_str(restore_plan_id).context("invalid restore plan UUID")?,
        archive_path: String::new(),
        archive_size_bytes: 0,
        archive_sha256_hex: String::new(),
        note: None,
        timeout_secs: 60,
        confirmed: false,
        force_unprivileged: false,
    };
    let mut index = 1;
    while index < tokens.len() {
        match tokens[index] {
            "--archive-path" => {
                request.archive_path = tokens
                    .get(index + 1)
                    .context("migration-run --archive-path requires a value")?
                    .to_string();
                index += 2;
            }
            "--archive-size-bytes" => {
                request.archive_size_bytes = tokens
                    .get(index + 1)
                    .context("migration-run --archive-size-bytes requires a value")?
                    .parse()
                    .context("invalid migration-run --archive-size-bytes")?;
                index += 2;
            }
            "--archive-sha256-hex" => {
                request.archive_sha256_hex = tokens
                    .get(index + 1)
                    .context("migration-run --archive-sha256-hex requires a value")?
                    .to_string();
                index += 2;
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
            "--timeout" => {
                request.timeout_secs = tokens
                    .get(index + 1)
                    .context("migration-run --timeout requires a value")?
                    .parse()
                    .context("invalid migration-run --timeout")?;
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
        (1..=3600).contains(&request.timeout_secs),
        "migration-run timeout out of range"
    );
    anyhow::ensure!(
        !request.archive_path.trim().is_empty(),
        "migration-run requires --archive-path"
    );
    anyhow::ensure!(
        request.archive_size_bytes > 0,
        "migration-run requires --archive-size-bytes"
    );
    anyhow::ensure!(
        !request.archive_sha256_hex.trim().is_empty(),
        "migration-run requires --archive-sha256-hex"
    );
    anyhow::ensure!(request.confirmed, "migration-run requires --confirmed");
    Ok(request)
}

pub(crate) fn submit_vty_migration_link(
    api_url: &str,
    token: Option<&str>,
    request: VtyMigrationLinkRequest,
) -> Result<String> {
    let body = json!({
        "restore_plan_id": request.restore_plan_id,
        "confirmed": request.confirmed,
        "note": request.note,
    });
    http_post_json(api_url, "/api/v1/migration-links", token, &body)
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
        request.archive_path,
        request.archive_size_bytes,
        request.archive_sha256_hex,
        request.note,
        &privilege_context.password,
        &privilege_context.salt_hex,
        300,
        request.timeout_secs,
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
        let request = parse_vty_migration_run(&[
            "49c7c3ea-0da8-40b6-b380-5543b1eb3adb",
            "--archive-path",
            "/var/lib/vpsman/restores/backup.tar",
            "--archive-size-bytes",
            "2048",
            "--archive-sha256-hex",
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "--note",
            "cutover",
            "--timeout",
            "120",
            "--force-unprivileged",
            "--confirmed",
        ])
        .unwrap();
        assert_eq!(
            request.restore_plan_id.to_string(),
            "49c7c3ea-0da8-40b6-b380-5543b1eb3adb"
        );
        assert_eq!(request.archive_path, "/var/lib/vpsman/restores/backup.tar");
        assert_eq!(request.archive_size_bytes, 2048);
        assert_eq!(
            request.archive_sha256_hex,
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        );
        assert_eq!(request.note.as_deref(), Some("cutover"));
        assert_eq!(request.timeout_secs, 120);
        assert!(request.force_unprivileged);
        assert!(request.confirmed);
    }

    #[test]
    fn rejects_unconfirmed_vty_migration_run() {
        assert!(parse_vty_migration_run(&["49c7c3ea-0da8-40b6-b380-5543b1eb3adb"]).is_err());
    }
}
