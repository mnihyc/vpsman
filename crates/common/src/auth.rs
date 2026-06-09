use std::collections::{HashSet, VecDeque};

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use hmac::{Hmac, Mac};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

type HmacSha256 = Hmac<Sha256>;

pub const MAX_PRIVILEGE_ASSERTION_AGE_SECS: u64 = 300;
pub const MAX_PRIVILEGE_ASSERTION_CLOCK_SKEW_SECS: u64 = 60;
pub const MAX_COMMAND_SIGNATURE_AGE_SECS: u64 = 300;
pub const MAX_COMMAND_SIGNATURE_CLOCK_SKEW_SECS: u64 = 60;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PrivilegeAssertion {
    pub nonce_hex: String,
    pub issued_unix: u64,
    pub expires_unix: u64,
    pub assertion_hex: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CommandEnvelope {
    pub command_id: Uuid,
    pub scope: String,
    pub payload_hash_hex: String,
    #[serde(default)]
    pub signed_unix: u64,
    #[serde(default)]
    pub expires_unix: u64,
    pub server_signature: Vec<u8>,
}

#[derive(Debug, thiserror::Error, Eq, PartialEq)]
pub enum AuthorizationError {
    #[error("command scope mismatch: expected {expected}, got {actual}")]
    ScopeMismatch { expected: String, actual: String },
    #[error("command payload hash mismatch")]
    PayloadHashMismatch,
    #[error("command server signature timestamp is invalid or expired")]
    InvalidCommandSignatureTime,
    #[error("command is missing server signature")]
    MissingServerSignature,
    #[error("command server signature is invalid")]
    InvalidServerSignature,
    #[error("command id was already used")]
    Replay,
}

#[derive(Debug)]
pub struct PrivilegeReplayCache {
    max_entries: usize,
    seen: HashSet<(Uuid, String)>,
    order: VecDeque<(Uuid, String)>,
}

impl Default for PrivilegeReplayCache {
    fn default() -> Self {
        Self::new(4096)
    }
}

impl PrivilegeReplayCache {
    pub fn new(max_entries: usize) -> Self {
        Self {
            max_entries: max_entries.max(1),
            seen: HashSet::new(),
            order: VecDeque::new(),
        }
    }

    fn remember(&mut self, command_id: Uuid, nonce_hex: &str) -> Result<(), AuthorizationError> {
        let key = (command_id, nonce_hex.to_string());
        if self.seen.contains(&key) {
            return Err(AuthorizationError::Replay);
        }

        self.seen.insert(key.clone());
        self.order.push_back(key);
        while self.order.len() > self.max_entries {
            if let Some(expired) = self.order.pop_front() {
                self.seen.remove(&expired);
            }
        }
        Ok(())
    }
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

pub fn sign_command_envelope(
    server_signing_key: &SigningKey,
    envelope: &CommandEnvelope,
) -> Vec<u8> {
    server_signing_key
        .sign(&command_signature_payload(envelope))
        .to_bytes()
        .to_vec()
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

pub fn verify_command_envelope(
    server_verifying_key: &VerifyingKey,
    expected_scope: &str,
    payload: &[u8],
    envelope: &CommandEnvelope,
    now_unix: u64,
    replay_cache: &mut PrivilegeReplayCache,
) -> Result<(), AuthorizationError> {
    if envelope.scope != expected_scope {
        return Err(AuthorizationError::ScopeMismatch {
            expected: expected_scope.to_string(),
            actual: envelope.scope.clone(),
        });
    }

    if envelope.payload_hash_hex != payload_hash(payload) {
        return Err(AuthorizationError::PayloadHashMismatch);
    }

    if envelope.expires_unix < envelope.signed_unix
        || envelope.expires_unix < now_unix
        || envelope.signed_unix > now_unix.saturating_add(MAX_COMMAND_SIGNATURE_CLOCK_SKEW_SECS)
        || now_unix.saturating_sub(envelope.signed_unix) > MAX_COMMAND_SIGNATURE_AGE_SECS
    {
        return Err(AuthorizationError::InvalidCommandSignatureTime);
    }

    if envelope.server_signature.is_empty() {
        return Err(AuthorizationError::MissingServerSignature);
    }
    let signature = Signature::from_slice(&envelope.server_signature)
        .map_err(|_| AuthorizationError::InvalidServerSignature)?;
    server_verifying_key
        .verify(&command_signature_payload(envelope), &signature)
        .map_err(|_| AuthorizationError::InvalidServerSignature)?;

    replay_cache.remember(envelope.command_id, "server-signature")
}

fn command_signature_payload(envelope: &CommandEnvelope) -> Vec<u8> {
    let mut payload = Vec::with_capacity(160);
    push_len_prefixed(&mut payload, b"vpsman-server-command-envelope-v1");
    payload.extend_from_slice(envelope.command_id.as_bytes());
    push_len_prefixed(&mut payload, envelope.scope.as_bytes());
    push_len_prefixed(&mut payload, envelope.payload_hash_hex.as_bytes());
    payload.extend_from_slice(&envelope.signed_unix.to_be_bytes());
    payload.extend_from_slice(&envelope.expires_unix.to_be_bytes());
    payload
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
    fn command_envelope_authorizes_once() {
        let signing = SigningKey::from_bytes(&[7_u8; 32]);
        let verifying = signing.verifying_key();
        let payload = br#"{"argv":["/bin/true"]}"#;
        let id = Uuid::new_v4();
        let hash = payload_hash(payload);
        let mut envelope = CommandEnvelope {
            command_id: id,
            scope: "client:lax-edge-01".to_string(),
            payload_hash_hex: hash,
            signed_unix: 100,
            expires_unix: 200,
            server_signature: Vec::new(),
        };
        envelope.server_signature = sign_command_envelope(&signing, &envelope);

        let mut replay_cache = PrivilegeReplayCache::default();
        assert_eq!(
            verify_command_envelope(
                &verifying,
                "client:lax-edge-01",
                payload,
                &envelope,
                100,
                &mut replay_cache,
            ),
            Ok(())
        );
        assert_eq!(
            verify_command_envelope(
                &verifying,
                "client:lax-edge-01",
                payload,
                &envelope,
                100,
                &mut replay_cache,
            ),
            Err(AuthorizationError::Replay)
        );
    }

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
    fn command_envelope_rejects_scope_payload_and_signature_mismatch() {
        let signing = SigningKey::from_bytes(&[9_u8; 32]);
        let verifying = signing.verifying_key();
        let payload = b"payload";
        let id = Uuid::new_v4();
        let hash = payload_hash(payload);
        let mut envelope = CommandEnvelope {
            command_id: id,
            scope: "client:one".to_string(),
            payload_hash_hex: hash,
            signed_unix: 100,
            expires_unix: 200,
            server_signature: Vec::new(),
        };
        envelope.server_signature = sign_command_envelope(&signing, &envelope);

        let mut replay_cache = PrivilegeReplayCache::default();
        assert_eq!(
            verify_command_envelope(
                &verifying,
                "client:two",
                payload,
                &envelope,
                100,
                &mut replay_cache,
            ),
            Err(AuthorizationError::ScopeMismatch {
                expected: "client:two".to_string(),
                actual: "client:one".to_string(),
            })
        );
        assert_eq!(
            verify_command_envelope(
                &verifying,
                "client:one",
                b"different",
                &envelope,
                100,
                &mut replay_cache,
            ),
            Err(AuthorizationError::PayloadHashMismatch)
        );

        let mut tampered = envelope;
        tampered.server_signature[0] ^= 0x01;
        assert_eq!(
            verify_command_envelope(
                &verifying,
                "client:one",
                payload,
                &tampered,
                100,
                &mut replay_cache,
            ),
            Err(AuthorizationError::InvalidServerSignature)
        );
    }

    #[test]
    fn command_envelope_rejects_expired_signature_time() {
        let signing = SigningKey::from_bytes(&[11_u8; 32]);
        let verifying = signing.verifying_key();
        let payload = b"payload";
        let id = Uuid::new_v4();
        let mut expired = CommandEnvelope {
            command_id: id,
            scope: "client:one".to_string(),
            payload_hash_hex: payload_hash(payload),
            signed_unix: 90,
            expires_unix: 99,
            server_signature: Vec::new(),
        };
        expired.server_signature = sign_command_envelope(&signing, &expired);
        let mut replay_cache = PrivilegeReplayCache::default();
        assert_eq!(
            verify_command_envelope(
                &verifying,
                "client:one",
                payload,
                &expired,
                100,
                &mut replay_cache,
            ),
            Err(AuthorizationError::InvalidCommandSignatureTime)
        );
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
