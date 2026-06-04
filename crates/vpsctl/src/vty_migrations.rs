use std::path::PathBuf;

use anyhow::{Context, Result};
use serde_json::json;
use uuid::Uuid;

use crate::{
    commands_migrations::migration_run_with_credentials, http::http_post_json,
    vty_jobs::VtyProofContext,
};

pub(crate) struct VtyMigrationLinkRequest {
    pub(crate) restore_plan_id: Uuid,
    pub(crate) note: Option<String>,
    pub(crate) confirmed: bool,
}

pub(crate) struct VtyMigrationRunRequest {
    pub(crate) restore_plan_id: Uuid,
    pub(crate) artifact_file: Option<PathBuf>,
    pub(crate) private_key_env: String,
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
        .context("usage: migration-run <restore_plan_uuid> [--artifact-file <path>] [--private-key-env <env>] [--note <text>] [--timeout <1-3600>] [--force-unprivileged] --confirmed")?;
    let mut request = VtyMigrationRunRequest {
        restore_plan_id: Uuid::parse_str(restore_plan_id).context("invalid restore plan UUID")?,
        artifact_file: None,
        private_key_env: "VPSMAN_BACKUP_PRIVATE_KEY_HEX".to_string(),
        note: None,
        timeout_secs: 60,
        confirmed: false,
        force_unprivileged: false,
    };
    let mut index = 1;
    while index < tokens.len() {
        match tokens[index] {
            "--artifact-file" => {
                request.artifact_file = Some(PathBuf::from(
                    tokens
                        .get(index + 1)
                        .context("migration-run --artifact-file requires a value")?,
                ));
                index += 2;
            }
            "--private-key-env" => {
                request.private_key_env = tokens
                    .get(index + 1)
                    .context("migration-run --private-key-env requires a value")?
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
    proof_context: &VtyProofContext,
    request: VtyMigrationRunRequest,
) -> Result<String> {
    migration_run_with_credentials(
        api_url,
        token,
        request.restore_plan_id,
        request.artifact_file,
        request.private_key_env,
        request.note,
        &proof_context.password,
        &proof_context.salt_hex,
        300,
        request.timeout_secs,
        request.confirmed,
        request.force_unprivileged,
    )
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

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
            "--artifact-file",
            "/tmp/backup.json",
            "--private-key-env",
            "RESTORE_KEY",
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
        assert_eq!(
            request.artifact_file.unwrap(),
            PathBuf::from("/tmp/backup.json")
        );
        assert_eq!(request.private_key_env, "RESTORE_KEY");
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
