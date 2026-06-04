use anyhow::{Context, Result};
use chacha20poly1305::{
    aead::{Aead, KeyInit},
    ChaCha20Poly1305, Nonce,
};
use hmac::{Hmac, Mac};
use rand::RngCore;
use sha1::Sha1;

use crate::security::{argon2_digest, constant_time_eq};

pub(crate) const TOTP_DIGITS: u8 = 6;
pub(crate) const TOTP_PERIOD_SECS: u64 = 30;
const TOTP_SECRET_BYTES: usize = 20;
const TOTP_AEAD_AAD: &[u8] = b"vpsman-operator-totp-v1";

#[derive(Clone, Debug)]
pub(crate) struct EncryptedTotpSecret {
    pub(crate) ciphertext_hex: String,
    pub(crate) nonce_hex: String,
    pub(crate) salt_hex: String,
}

pub(crate) fn encrypt_new_totp_secret(password: &str) -> Result<(Vec<u8>, EncryptedTotpSecret)> {
    let mut secret = vec![0_u8; TOTP_SECRET_BYTES];
    rand::thread_rng().fill_bytes(&mut secret);
    let encrypted = encrypt_totp_secret(password, &secret)?;
    Ok((secret, encrypted))
}

pub(crate) fn encrypt_totp_secret(password: &str, secret: &[u8]) -> Result<EncryptedTotpSecret> {
    anyhow::ensure!(
        secret.len() >= 16 && secret.len() <= 64,
        "TOTP secret length out of range"
    );
    let mut salt = [0_u8; 16];
    let mut nonce = [0_u8; 12];
    rand::thread_rng().fill_bytes(&mut salt);
    rand::thread_rng().fill_bytes(&mut nonce);
    let key = argon2_digest(password.as_bytes(), &salt)?;
    let cipher = ChaCha20Poly1305::new((&key).into());
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(&nonce),
            chacha20poly1305::aead::Payload {
                msg: secret,
                aad: TOTP_AEAD_AAD,
            },
        )
        .map_err(|_| anyhow::anyhow!("failed to encrypt TOTP secret"))?;
    Ok(EncryptedTotpSecret {
        ciphertext_hex: hex::encode(ciphertext),
        nonce_hex: hex::encode(nonce),
        salt_hex: hex::encode(salt),
    })
}

pub(crate) fn decrypt_totp_secret(
    password: &str,
    encrypted: &EncryptedTotpSecret,
) -> Result<Vec<u8>> {
    let ciphertext = hex::decode(&encrypted.ciphertext_hex).context("invalid TOTP ciphertext")?;
    let nonce = hex::decode(&encrypted.nonce_hex).context("invalid TOTP nonce")?;
    let salt = hex::decode(&encrypted.salt_hex).context("invalid TOTP salt")?;
    anyhow::ensure!(nonce.len() == 12, "invalid TOTP nonce length");
    anyhow::ensure!(salt.len() == 16, "invalid TOTP salt length");
    let key = argon2_digest(password.as_bytes(), &salt)?;
    let cipher = ChaCha20Poly1305::new((&key).into());
    cipher
        .decrypt(
            Nonce::from_slice(&nonce),
            chacha20poly1305::aead::Payload {
                msg: &ciphertext,
                aad: TOTP_AEAD_AAD,
            },
        )
        .map_err(|_| anyhow::anyhow!("invalid TOTP password or ciphertext"))
}

pub(crate) fn verify_totp_code(secret: &[u8], code: &str, now_unix: u64) -> bool {
    let Some(code) = normalize_totp_code(code) else {
        return false;
    };
    let current_step = now_unix / TOTP_PERIOD_SECS;
    [
        current_step.saturating_sub(1),
        current_step,
        current_step.saturating_add(1),
    ]
    .into_iter()
    .any(|step| constant_time_eq(totp_code_for_step(secret, step).as_bytes(), code.as_bytes()))
}

pub(crate) fn totp_code_for_step(secret: &[u8], step: u64) -> String {
    let mut mac = <Hmac<Sha1> as Mac>::new_from_slice(secret)
        .expect("HMAC accepts TOTP secrets of any length");
    mac.update(&step.to_be_bytes());
    let digest = mac.finalize().into_bytes();
    let offset = (digest[19] & 0x0f) as usize;
    let binary = (((digest[offset] & 0x7f) as u32) << 24)
        | ((digest[offset + 1] as u32) << 16)
        | ((digest[offset + 2] as u32) << 8)
        | (digest[offset + 3] as u32);
    let divisor = 10_u32.pow(TOTP_DIGITS as u32);
    format!("{:0width$}", binary % divisor, width = TOTP_DIGITS as usize)
}

pub(crate) fn base32_no_padding(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut output = String::new();
    let mut buffer = 0_u16;
    let mut bits = 0_u8;
    for byte in bytes {
        buffer = (buffer << 8) | (*byte as u16);
        bits += 8;
        while bits >= 5 {
            let index = ((buffer >> (bits - 5)) & 0x1f) as usize;
            output.push(ALPHABET[index] as char);
            bits -= 5;
        }
    }
    if bits > 0 {
        let index = ((buffer << (5 - bits)) & 0x1f) as usize;
        output.push(ALPHABET[index] as char);
    }
    output
}

pub(crate) fn otpauth_uri(username: &str, secret_base32: &str) -> String {
    let issuer = "vpsman";
    format!(
        "otpauth://totp/{issuer}:{}?secret={secret_base32}&issuer={issuer}&algorithm=SHA1&digits={TOTP_DIGITS}&period={TOTP_PERIOD_SECS}",
        percent_encode(username)
    )
}

fn normalize_totp_code(code: &str) -> Option<String> {
    let code = code.trim().replace(' ', "");
    if code.len() == TOTP_DIGITS as usize && code.bytes().all(|byte| byte.is_ascii_digit()) {
        Some(code)
    } else {
        None
    }
}

fn percent_encode(input: &str) -> String {
    input
        .bytes()
        .flat_map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
                vec![byte as char]
            } else {
                format!("%{byte:02X}").chars().collect()
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn totp_matches_rfc6238_sha1_vector() {
        let secret = b"12345678901234567890";

        assert_eq!(totp_code_for_step(secret, 59 / 30), "287082");
        assert_eq!(totp_code_for_step(secret, 1_111_111_109 / 30), "081804");
    }

    #[test]
    fn encrypted_totp_secret_requires_password_and_preserves_secret() {
        let secret = b"local totp secret bytes";
        let encrypted = encrypt_totp_secret("operator-password-123", secret).unwrap();

        assert!(!encrypted.ciphertext_hex.contains("local"));
        assert_eq!(
            decrypt_totp_secret("operator-password-123", &encrypted).unwrap(),
            secret
        );
        assert!(decrypt_totp_secret("wrong-password-123", &encrypted).is_err());
    }

    #[test]
    fn totp_verifier_accepts_one_step_clock_skew_and_rejects_bad_shape() {
        let secret = b"12345678901234567890";
        let code = totp_code_for_step(secret, 1_111_111_109 / 30);

        assert!(verify_totp_code(secret, &code, 1_111_111_109));
        assert!(verify_totp_code(secret, &code, 1_111_111_109 + 30));
        assert!(!verify_totp_code(secret, "12345", 1_111_111_109));
        assert!(!verify_totp_code(secret, "abcdef", 1_111_111_109));
    }
}
