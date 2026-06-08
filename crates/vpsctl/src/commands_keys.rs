use anyhow::Result;
use ed25519_dalek::SigningKey;
use rand::RngCore;
use vpsman_common::generate_noise_keypair;

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
