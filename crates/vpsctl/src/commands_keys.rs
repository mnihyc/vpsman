use anyhow::{bail, Result};
use ed25519_dalek::SigningKey;
use rand::RngCore;
use vpsman_common::generate_noise_keypair;

use crate::{
    http::{http_get, http_post_json},
    privilege::{
        derive_privilege_verifier_key_hex, load_super_password, load_super_salt_hex,
        random_super_salt_hex,
    },
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

pub(crate) fn signing_keygen() -> Result<()> {
    let mut seed = [0_u8; 32];
    rand::thread_rng().fill_bytes(&mut seed);
    let signing_key = SigningKey::from_bytes(&seed);
    println!(
        "{}",
        serde_json::json!({
            "private_key_hex": hex::encode(signing_key.to_bytes()),
            "public_key_hex": hex::encode(signing_key.verifying_key().to_bytes())
        })
    );
    Ok(())
}

#[derive(Debug, clap::Args)]
pub(crate) struct PrivilegeVerifierCommand {
    #[arg(
        long,
        default_value = "VPSMAN_SUPER_PASSWORD",
        help = "Environment variable that contains the operator super password"
    )]
    pub(crate) password_env: String,
    #[arg(
        long,
        help = "Existing operator salt as hex; defaults to VPSMAN_SUPER_SALT_HEX when --generate-salt is not set"
    )]
    pub(crate) super_salt_hex: Option<String>,
    #[arg(
        long,
        default_value_t = false,
        help = "Generate a new random 32-byte operator salt and include it in the output"
    )]
    pub(crate) generate_salt: bool,
}

pub(crate) fn privilege_verifier(command: PrivilegeVerifierCommand) -> Result<()> {
    let password = load_super_password(&command.password_env)?;
    let salt_hex = match (command.super_salt_hex.as_deref(), command.generate_salt) {
        (Some(_), true) => bail!("use either --super-salt-hex or --generate-salt, not both"),
        (Some(salt_hex), false) => load_super_salt_hex(Some(salt_hex))?,
        (None, true) => random_super_salt_hex(),
        (None, false) => load_super_salt_hex(None)?,
    };
    let verifier_hex = derive_privilege_verifier_key_hex(&password, &salt_hex)?;

    println!(
        "{}",
        serde_json::json!({
            "super_salt_hex": salt_hex.clone(),
            "privilege_verifier_key_hex": verifier_hex.clone(),
            "gateway_env": {
                "VPSMAN_PRIVILEGE_VERIFIER_KEY_HEX": verifier_hex,
            },
            "operator_env": {
                "VPSMAN_SUPER_SALT_HEX": salt_hex,
            },
            "api_env": {
                "VPSMAN_PRIVILEGE_VERIFIER_KEY_HEX": null,
                "VPSMAN_SUPER_PASSWORD": null,
                "VPSMAN_SUPER_SALT_HEX": null,
            },
            "password_source_env": command.password_env,
            "salt_generated": command.generate_salt,
            "algorithm": "sha256(vpsman-super-key-v1 || uint64_be(len(salt_bytes)) || salt_bytes || utf8(super_password))",
            "notes": [
                "paste only privilege_verifier_key_hex into gateway env",
                "keep the super password and super_salt_hex only in operator-side panel/CLI unlock material",
                "never pass password, salt, or verifier to agents; never pass verifier to API"
            ]
        })
    );
    Ok(())
}

pub(crate) struct AgentIdentityUpsertOptions {
    pub(crate) client_id: Option<String>,
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
    let mut body = serde_json::json!({
        "client_public_key_hex": options.client_public_key_hex,
        "display_name": options.display_name,
        "tags": options.tags,
        "replace_existing_key": options.replace_existing_key,
        "confirmed": options.confirmed,
    });
    if let Some(client_id) = options.client_id.as_deref() {
        body["client_id"] = serde_json::Value::String(client_id.to_string());
    }
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
    println!(
        "{}",
        http_post_json(
            api_url,
            &format!("/api/v1/clients/{client_id}/key-revocations"),
            token,
            &serde_json::json!({
                "reason": reason,
                "confirmed": confirmed,
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
