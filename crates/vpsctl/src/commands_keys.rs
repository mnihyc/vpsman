use anyhow::{Context, Result};
use ed25519_dalek::SigningKey;
use rand::RngCore;
use uuid::Uuid;
use vpsman_common::{
    derive_super_key, generate_noise_keypair, random_nonce, sign_privilege_proof, CommandEnvelope,
};

use crate::util::{ensure_payload_hash, unix_now};

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

pub(crate) fn print_proof(
    scope: &str,
    salt_hex: &str,
    payload_hash_hex: &str,
    password_env: &str,
    command_id: Option<&str>,
    ttl_secs: u64,
) -> Result<()> {
    let salt = hex::decode(salt_hex).context("invalid --salt-hex")?;
    ensure_payload_hash(payload_hash_hex)?;
    let password = std::env::var(password_env)
        .with_context(|| format!("environment variable {password_env} is not set"))?;
    let super_key = derive_super_key(&password, &salt);
    let command_id = match command_id {
        Some(value) => Uuid::parse_str(value).context("invalid --command-id")?,
        None => Uuid::new_v4(),
    };
    let expires_unix = unix_now().saturating_add(ttl_secs.max(1));
    let nonce = random_nonce();
    let proof = sign_privilege_proof(
        &super_key,
        command_id,
        scope,
        payload_hash_hex,
        &nonce,
        expires_unix,
    );
    let envelope = CommandEnvelope {
        command_id,
        scope: scope.to_string(),
        payload_hash_hex: payload_hash_hex.to_string(),
        proof: Some(proof),
        server_signature: Vec::new(),
    };

    println!("{}", serde_json::to_string_pretty(&envelope)?);
    Ok(())
}
