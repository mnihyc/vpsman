use anyhow::Result;
use ed25519_dalek::SigningKey;
use rand::RngCore;
use vpsman_common::generate_noise_keypair;

use crate::http::{http_get, http_post_json};

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
