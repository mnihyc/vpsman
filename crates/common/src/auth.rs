use std::collections::{HashSet, VecDeque};

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use hmac::{Hmac, Mac};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

pub const MAX_PRIVILEGE_ASSERTION_AGE_SECS: u64 = 300;
pub const MAX_PRIVILEGE_ASSERTION_CLOCK_SKEW_SECS: u64 = 60;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PrivilegeAssertion {
    pub nonce_hex: String,
    pub issued_unix: u64,
    pub expires_unix: u64,
    pub assertion_hex: String,
}

#[derive(Debug, thiserror::Error, Eq, PartialEq)]
pub enum PrivilegeAssertionError {
    #[error("privilege assertion nonce is invalid")]
    InvalidNonce,
    #[error("privilege assertion timestamp is invalid or expired")]
    InvalidTime,
    #[error("privilege assertion HMAC is invalid")]
    InvalidAssertion,
    #[error("privilege assertion nonce was already used")]
    Replay,
}

#[derive(Debug)]
pub struct PrivilegeAssertionReplayCache {
    max_entries: usize,
    seen: HashSet<String>,
    order: VecDeque<String>,
}

impl Default for PrivilegeAssertionReplayCache {
    fn default() -> Self {
        Self::new(4096)
    }
}

impl PrivilegeAssertionReplayCache {
    pub fn new(max_entries: usize) -> Self {
        Self {
            max_entries: max_entries.max(1),
            seen: HashSet::new(),
            order: VecDeque::new(),
        }
    }

    pub fn remember(&mut self, nonce_hex: &str) -> Result<(), PrivilegeAssertionError> {
        let nonce_hex = nonce_hex.to_string();
        if self.seen.contains(&nonce_hex) {
            return Err(PrivilegeAssertionError::Replay);
        }
        self.seen.insert(nonce_hex.clone());
        self.order.push_back(nonce_hex);
        while self.order.len() > self.max_entries {
            if let Some(expired) = self.order.pop_front() {
                self.seen.remove(&expired);
            }
        }
        Ok(())
    }
}

pub fn random_nonce() -> [u8; 16] {
    let mut nonce = [0_u8; 16];
    rand::thread_rng().fill_bytes(&mut nonce);
    nonce
}

pub fn derive_super_key(password: &str, salt: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"vpsman-super-key-v1");
    hasher.update((salt.len() as u64).to_be_bytes());
    hasher.update(salt);
    hasher.update(password.as_bytes());
    hasher.finalize().into()
}

pub fn payload_hash(payload: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(payload);
    hex::encode(hasher.finalize())
}

pub fn sign_privilege_assertion(
    verifier_key: &[u8; 32],
    intent_hash_hex: &str,
    nonce: &[u8; 16],
    issued_unix: u64,
    expires_unix: u64,
) -> PrivilegeAssertion {
    let mut mac = HmacSha256::new_from_slice(verifier_key).expect("HMAC accepts 32-byte keys");
    mac.update(b"vpsman-gateway-privilege-assertion-v1");
    mac.update(intent_hash_hex.as_bytes());
    mac.update(nonce);
    mac.update(&issued_unix.to_be_bytes());
    mac.update(&expires_unix.to_be_bytes());

    PrivilegeAssertion {
        nonce_hex: hex::encode(nonce),
        issued_unix,
        expires_unix,
        assertion_hex: hex::encode(mac.finalize().into_bytes()),
    }
}

pub fn verify_privilege_assertion(
    verifier_key: &[u8; 32],
    intent: &str,
    assertion: &PrivilegeAssertion,
    now_unix: u64,
    replay_cache: &mut PrivilegeAssertionReplayCache,
) -> Result<String, PrivilegeAssertionError> {
    if assertion.expires_unix < assertion.issued_unix
        || assertion.expires_unix < now_unix
        || assertion.issued_unix > now_unix.saturating_add(MAX_PRIVILEGE_ASSERTION_CLOCK_SKEW_SECS)
        || now_unix.saturating_sub(assertion.issued_unix) > MAX_PRIVILEGE_ASSERTION_AGE_SECS
    {
        return Err(PrivilegeAssertionError::InvalidTime);
    }
    let nonce_vec =
        hex::decode(&assertion.nonce_hex).map_err(|_| PrivilegeAssertionError::InvalidNonce)?;
    let nonce = <[u8; 16]>::try_from(nonce_vec.as_slice())
        .map_err(|_| PrivilegeAssertionError::InvalidNonce)?;
    let intent_hash_hex = payload_hash(intent.as_bytes());
    let expected = sign_privilege_assertion(
        verifier_key,
        &intent_hash_hex,
        &nonce,
        assertion.issued_unix,
        assertion.expires_unix,
    );
    if !constant_time_eq(
        expected.assertion_hex.as_bytes(),
        assertion.assertion_hex.as_bytes(),
    ) {
        return Err(PrivilegeAssertionError::InvalidAssertion);
    }
    replay_cache.remember(&assertion.nonce_hex)?;
    Ok(intent_hash_hex)
}

pub fn sign_update_artifact_hash(signing_key: &SigningKey, sha256_hex: &str) -> Vec<u8> {
    signing_key
        .sign(&update_artifact_signature_payload(sha256_hex))
        .to_bytes()
        .to_vec()
}

pub fn verify_update_artifact_signature(
    artifact_signing_key_hex: &str,
    artifact_signature_hex: &str,
    sha256_hex: &str,
) -> bool {
    let Ok(key_bytes) = hex::decode(artifact_signing_key_hex) else {
        return false;
    };
    let Ok(key_bytes) = <[u8; 32]>::try_from(key_bytes.as_slice()) else {
        return false;
    };
    let Ok(verifying_key) = VerifyingKey::from_bytes(&key_bytes) else {
        return false;
    };
    let Ok(signature_bytes) = hex::decode(artifact_signature_hex) else {
        return false;
    };
    let Ok(signature) = Signature::from_slice(&signature_bytes) else {
        return false;
    };
    verifying_key
        .verify(&update_artifact_signature_payload(sha256_hex), &signature)
        .is_ok()
}

fn update_artifact_signature_payload(sha256_hex: &str) -> Vec<u8> {
    let mut payload = Vec::with_capacity(96);
    push_len_prefixed(&mut payload, b"vpsman-update-artifact-v1");
    push_len_prefixed(&mut payload, sha256_hex.as_bytes());
    payload
}

fn push_len_prefixed(dst: &mut Vec<u8>, value: &[u8]) {
    dst.extend_from_slice(&(value.len() as u32).to_be_bytes());
    dst.extend_from_slice(value);
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter()
        .zip(b.iter())
        .fold(0_u8, |acc, (left, right)| acc | (left ^ right))
        == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_artifact_signature_rejects_hash_tampering() {
        let signing = SigningKey::from_bytes(&[23_u8; 32]);
        let public_key_hex = hex::encode(signing.verifying_key().to_bytes());
        let sha256_hex = "ab".repeat(32);
        let signature_hex = hex::encode(sign_update_artifact_hash(&signing, &sha256_hex));

        assert!(verify_update_artifact_signature(
            &public_key_hex,
            &signature_hex,
            &sha256_hex
        ));
        assert!(!verify_update_artifact_signature(
            &public_key_hex,
            &signature_hex,
            &"cd".repeat(32)
        ));
        assert!(!verify_update_artifact_signature(
            &"00".repeat(32),
            &signature_hex,
            &sha256_hex
        ));
    }

    #[test]
    fn privilege_assertion_authorizes_once_for_exact_intent() {
        let verifier_key = [3_u8; 32];
        let intent = r#"{"action":"job.dispatch","target":"client-a"}"#;
        let intent_hash = payload_hash(intent.as_bytes());
        let assertion =
            sign_privilege_assertion(&verifier_key, &intent_hash, &[7_u8; 16], 100, 300);
        let mut replay_cache = PrivilegeAssertionReplayCache::default();

        assert_eq!(
            verify_privilege_assertion(&verifier_key, intent, &assertion, 120, &mut replay_cache),
            Ok(intent_hash)
        );
        assert_eq!(
            verify_privilege_assertion(&verifier_key, intent, &assertion, 120, &mut replay_cache),
            Err(PrivilegeAssertionError::Replay)
        );
    }

    #[test]
    fn privilege_assertion_rejects_mismatched_and_stale_intent() {
        let verifier_key = [4_u8; 32];
        let intent = r#"{"action":"tag.delete","target":"tag:prod"}"#;
        let intent_hash = payload_hash(intent.as_bytes());
        let assertion =
            sign_privilege_assertion(&verifier_key, &intent_hash, &[8_u8; 16], 100, 300);
        let mut replay_cache = PrivilegeAssertionReplayCache::default();

        assert_eq!(
            verify_privilege_assertion(
                &verifier_key,
                r#"{"action":"tag.delete","target":"tag:stage"}"#,
                &assertion,
                120,
                &mut replay_cache
            ),
            Err(PrivilegeAssertionError::InvalidAssertion)
        );

        let stale = sign_privilege_assertion(&verifier_key, &intent_hash, &[9_u8; 16], 100, 1000);
        assert_eq!(
            verify_privilege_assertion(
                &verifier_key,
                intent,
                &stale,
                401,
                &mut PrivilegeAssertionReplayCache::default()
            ),
            Err(PrivilegeAssertionError::InvalidTime)
        );
    }
}
