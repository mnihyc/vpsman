use std::{
    fs::File,
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Result};
use rand::RngCore;
use vpsman_common::{
    create_private_file_new, derive_super_key, ensure_private_dir, generate_noise_keypair,
    write_private_file_atomically,
};

use crate::http::{http_get, http_post_json};
use crate::privilege::{
    build_privilege_for_db, decode_super_salt, load_super_password, load_super_salt_hex,
    DbPrivilegeRequest,
};

pub(crate) fn noise_keygen() -> Result<()> {
    let keypair = generate_noise_keypair()?;
    println!(
        "{}",
        serde_json::json!({
            "private_key_hex": keypair.private_hex(),
            "public_key_hex": keypair.public_hex()
        })
    );
    Ok(())
}

pub(crate) struct AgentIdentityUpsertOptions {
    pub(crate) client_id: String,
    pub(crate) client_public_key_hex: String,
    pub(crate) display_name: Option<String>,
    pub(crate) tags: Vec<String>,
    pub(crate) replace_existing_key: bool,
    pub(crate) confirmed: bool,
}

pub(crate) fn agent_identity_upsert(
    api_url: &str,
    token: Option<&str>,
    options: AgentIdentityUpsertOptions,
) -> Result<()> {
    let password = load_super_password("VPSMAN_SUPER_PASSWORD")?;
    let salt_hex = load_super_salt_hex(None)?;
    let targets = vec![options.client_id.clone()];
    let privilege_assertion = build_privilege_for_db(
        DbPrivilegeRequest {
            action: if options.replace_existing_key {
                "agent_identity.rotate"
            } else {
                "agent_identity.import"
            },
            target: &options.client_id,
            selector_expression: None,
            resolved_targets: &targets,
            confirmed: options.confirmed,
            payload_hash: None,
        },
        &password,
        &salt_hex,
        300,
    )?;
    let body = serde_json::json!({
        "client_id": options.client_id,
        "client_public_key_hex": options.client_public_key_hex,
        "display_name": options.display_name,
        "tags": options.tags,
        "replace_existing_key": options.replace_existing_key,
        "confirmed": options.confirmed,
        "privilege_assertion": privilege_assertion,
    });
    println!(
        "{}",
        http_post_json(api_url, "/api/v1/agent-identities", token, &body,)?
    );
    Ok(())
}

pub(crate) fn client_key_revocations(api_url: &str, token: Option<&str>, limit: u16) -> Result<()> {
    println!(
        "{}",
        http_get(
            api_url,
            &format!("/api/v1/client-key-revocations?limit={}", limit),
            token,
        )?
    );
    Ok(())
}

pub(crate) fn client_key_revoke(
    api_url: &str,
    token: Option<&str>,
    client_id: String,
    reason: Option<String>,
    confirmed: bool,
) -> Result<()> {
    let password = load_super_password("VPSMAN_SUPER_PASSWORD")?;
    let salt_hex = load_super_salt_hex(None)?;
    let targets = vec![client_id.clone()];
    let privilege_assertion = build_privilege_for_db(
        DbPrivilegeRequest {
            action: "client_key.revoke",
            target: &client_id,
            selector_expression: None,
            resolved_targets: &targets,
            confirmed,
            payload_hash: None,
        },
        &password,
        &salt_hex,
        300,
    )?;
    println!(
        "{}",
        http_post_json(
            api_url,
            &format!("/api/v1/clients/{client_id}/key-revocations"),
            token,
            &serde_json::json!({
                "reason": reason,
                "confirmed": confirmed,
                "privilege_assertion": privilege_assertion,
            }),
        )?
    );
    Ok(())
}

pub(crate) fn key_lifecycle_report(api_url: &str, token: Option<&str>) -> Result<()> {
    println!(
        "{}",
        http_get(api_url, "/api/v1/key-lifecycle/report", token)?
    );
    Ok(())
}

pub(crate) struct ComposeSecretsOptions {
    pub(crate) secrets_dir: PathBuf,
    pub(crate) password_env: String,
    pub(crate) super_salt_hex: Option<String>,
    pub(crate) force: bool,
}

pub(crate) fn compose_secrets(options: ComposeSecretsOptions) -> Result<()> {
    ensure_private_dir(&options.secrets_dir).with_context(|| {
        format!(
            "failed to create compose secrets directory {}",
            options.secrets_dir.display()
        )
    })?;
    let internal_token_path = options.secrets_dir.join("vpsman_internal_token");
    let gateway_private_key_path = options.secrets_dir.join("vpsman_gateway_private_key_hex");
    let privilege_verifier_path = options
        .secrets_dir
        .join("vpsman_privilege_verifier_key_hex");
    let gateway_public_key_path = options.secrets_dir.join("vpsman_gateway_public_key_hex");
    let operator_env_path = options.secrets_dir.join("operator-privilege.env");
    let target_paths = [
        &internal_token_path,
        &gateway_private_key_path,
        &privilege_verifier_path,
        &gateway_public_key_path,
        &operator_env_path,
    ];
    if !options.force {
        for path in target_paths {
            if path
                .try_exists()
                .with_context(|| format!("failed to inspect {}", path.display()))?
            {
                bail!(
                    "{} already exists; pass --force to replace the compose secret set",
                    path.display()
                );
            }
        }
    }

    let password = load_super_password(&options.password_env)?;
    let super_salt_hex = match options.super_salt_hex {
        Some(value) => load_super_salt_hex(Some(&value))?,
        None => random_hex_32(),
    };
    let salt = decode_super_salt(&super_salt_hex)?;
    let privilege_verifier_key_hex = hex::encode(derive_super_key(&password, &salt));
    let internal_token = random_hex_32();
    let gateway_keypair = generate_noise_keypair()?;

    let mut written = Vec::new();
    write_compose_secret(
        &internal_token_path,
        &internal_token,
        options.force,
        &mut written,
    )?;
    write_compose_secret(
        &gateway_private_key_path,
        &gateway_keypair.private_hex(),
        options.force,
        &mut written,
    )?;
    write_compose_secret(
        &privilege_verifier_path,
        &privilege_verifier_key_hex,
        options.force,
        &mut written,
    )?;
    write_compose_secret(
        &gateway_public_key_path,
        &gateway_keypair.public_hex(),
        options.force,
        &mut written,
    )?;
    write_compose_secret(
        &operator_env_path,
        &format!("export VPSMAN_SUPER_SALT_HEX={super_salt_hex}"),
        options.force,
        &mut written,
    )?;

    println!(
        "{}",
        serde_json::json!({
            "compose_secrets": "ok",
            "secrets_dir": options.secrets_dir,
            "created_or_replaced": written,
            "operator_env_file": operator_env_path,
            "gateway_public_key_file": gateway_public_key_path,
            "required_runtime_files": [
                "vpsman_internal_token",
                "vpsman_gateway_private_key_hex",
                "vpsman_privilege_verifier_key_hex"
            ],
            "super_password_source": options.password_env,
        })
    );
    Ok(())
}

fn write_compose_secret(
    path: &Path,
    value: &str,
    force: bool,
    written: &mut Vec<String>,
) -> Result<()> {
    let contents = format!("{}\n", value.trim());
    if force {
        write_private_file_atomically(path, contents.as_bytes())
            .with_context(|| format!("failed to write {}", path.display()))?;
    } else {
        let mut file = create_private_file_new(path)
            .with_context(|| format!("failed to create {}", path.display()))?;
        file.write_all(contents.as_bytes())
            .with_context(|| format!("failed to write {}", path.display()))?;
        sync_file(file, path)?;
    }
    written.push(path.to_string_lossy().into_owned());
    Ok(())
}

fn sync_file(file: File, path: &Path) -> Result<()> {
    file.sync_all()
        .with_context(|| format!("failed to sync {}", path.display()))
}

fn random_hex_32() -> String {
    let mut bytes = [0_u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt};

    #[test]
    fn compose_secrets_write_required_files_without_exposing_password() {
        let root = std::env::temp_dir().join(format!(
            "vpsctl-compose-secrets-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let password_env = "VPSCTL_TEST_COMPOSE_SECRET_PASSWORD";
        std::env::set_var(password_env, "correct horse battery staple");
        compose_secrets(ComposeSecretsOptions {
            secrets_dir: root.clone(),
            password_env: password_env.to_string(),
            super_salt_hex: Some("01020304".to_string()),
            force: false,
        })
        .unwrap();

        for name in [
            "vpsman_internal_token",
            "vpsman_gateway_private_key_hex",
            "vpsman_privilege_verifier_key_hex",
            "vpsman_gateway_public_key_hex",
            "operator-privilege.env",
        ] {
            let path = root.join(name);
            let contents = fs::read_to_string(&path).unwrap();
            assert!(!contents.contains("correct horse battery staple"));
            let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        assert_eq!(
            fs::read_to_string(root.join("operator-privilege.env")).unwrap(),
            "export VPSMAN_SUPER_SALT_HEX=01020304\n"
        );
        let expected_verifier = hex::encode(derive_super_key(
            "correct horse battery staple",
            &[1, 2, 3, 4],
        ));
        assert_eq!(
            fs::read_to_string(root.join("vpsman_privilege_verifier_key_hex")).unwrap(),
            format!("{expected_verifier}\n")
        );
        let _ = fs::remove_dir_all(root);
        std::env::remove_var(password_env);
    }
}
