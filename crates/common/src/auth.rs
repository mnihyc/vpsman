use std::collections::{HashSet, VecDeque};

use crate::DiscoveryDocument;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use hmac::{Hmac, Mac};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PrivilegeProof {
    pub nonce_hex: String,
    pub expires_unix: u64,
    pub proof_hex: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CommandEnvelope {
    pub command_id: Uuid,
    pub scope: String,
    pub payload_hash_hex: String,
    pub proof: Option<PrivilegeProof>,
    pub server_signature: Vec<u8>,
}

#[derive(Debug, thiserror::Error, Eq, PartialEq)]
pub enum AuthorizationError {
    #[error("command scope mismatch: expected {expected}, got {actual}")]
    ScopeMismatch { expected: String, actual: String },
    #[error("command payload hash mismatch")]
    PayloadHashMismatch,
    #[error("command is missing super-password proof")]
    MissingProof,
    #[error("command super-password proof is invalid or expired")]
    InvalidProof,
    #[error("command is missing server signature")]
    MissingServerSignature,
    #[error("command server signature is invalid")]
    InvalidServerSignature,
    #[error("command proof nonce was already used")]
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

pub fn sign_privilege_proof(
    super_key: &[u8; 32],
    command_id: Uuid,
    scope: &str,
    payload_hash_hex: &str,
    nonce: &[u8; 16],
    expires_unix: u64,
) -> PrivilegeProof {
    let mut mac = HmacSha256::new_from_slice(super_key).expect("HMAC accepts 32-byte keys");
    mac.update(b"vpsman-privileged-command-v1");
    mac.update(command_id.as_bytes());
    mac.update(scope.as_bytes());
    mac.update(payload_hash_hex.as_bytes());
    mac.update(nonce);
    mac.update(&expires_unix.to_be_bytes());

    PrivilegeProof {
        nonce_hex: hex::encode(nonce),
        expires_unix,
        proof_hex: hex::encode(mac.finalize().into_bytes()),
    }
}

pub fn verify_privilege_proof(
    super_key: &[u8; 32],
    command_id: Uuid,
    scope: &str,
    payload_hash_hex: &str,
    proof: &PrivilegeProof,
    now_unix: u64,
) -> bool {
    if proof.expires_unix < now_unix {
        return false;
    }
    let Ok(nonce_vec) = hex::decode(&proof.nonce_hex) else {
        return false;
    };
    let Ok(nonce) = <[u8; 16]>::try_from(nonce_vec.as_slice()) else {
        return false;
    };
    let expected = sign_privilege_proof(
        super_key,
        command_id,
        scope,
        payload_hash_hex,
        &nonce,
        proof.expires_unix,
    );
    constant_time_eq(expected.proof_hex.as_bytes(), proof.proof_hex.as_bytes())
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

pub fn sign_discovery_document(
    server_signing_key: &SigningKey,
    document: &DiscoveryDocument,
) -> Vec<u8> {
    server_signing_key
        .sign(&discovery_signature_payload(document))
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

pub fn verify_discovery_document_signature(
    server_verifying_key: &VerifyingKey,
    document: &DiscoveryDocument,
) -> bool {
    let Ok(signature) = Signature::from_slice(&document.signature) else {
        return false;
    };
    server_verifying_key
        .verify(&discovery_signature_payload(document), &signature)
        .is_ok()
}

pub fn verify_command_envelope(
    super_key: &[u8; 32],
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

    let proof = envelope
        .proof
        .as_ref()
        .ok_or(AuthorizationError::MissingProof)?;
    if !verify_privilege_proof(
        super_key,
        envelope.command_id,
        &envelope.scope,
        &envelope.payload_hash_hex,
        proof,
        now_unix,
    ) {
        return Err(AuthorizationError::InvalidProof);
    }

    if envelope.server_signature.is_empty() {
        return Err(AuthorizationError::MissingServerSignature);
    }
    let signature = Signature::from_slice(&envelope.server_signature)
        .map_err(|_| AuthorizationError::InvalidServerSignature)?;
    server_verifying_key
        .verify(&command_signature_payload(envelope), &signature)
        .map_err(|_| AuthorizationError::InvalidServerSignature)?;

    replay_cache.remember(envelope.command_id, &proof.nonce_hex)
}

fn discovery_signature_payload(document: &DiscoveryDocument) -> Vec<u8> {
    let mut payload = Vec::with_capacity(128 + document.endpoints.len() * 64);
    push_len_prefixed(&mut payload, b"vpsman-discovery-document-v1");
    payload.extend_from_slice(&document.version.to_be_bytes());
    payload.extend_from_slice(&document.issued_unix.to_be_bytes());
    payload.extend_from_slice(&document.expires_unix.to_be_bytes());
    payload.extend_from_slice(&(document.endpoints.len() as u32).to_be_bytes());
    for endpoint in &document.endpoints {
        push_len_prefixed(&mut payload, endpoint.label.as_bytes());
        push_len_prefixed(&mut payload, endpoint.tcp_addr.as_bytes());
        payload.extend_from_slice(&endpoint.priority.to_be_bytes());
    }
    payload
}

fn command_signature_payload(envelope: &CommandEnvelope) -> Vec<u8> {
    let mut payload = Vec::with_capacity(160);
    push_len_prefixed(&mut payload, b"vpsman-server-command-envelope-v1");
    payload.extend_from_slice(envelope.command_id.as_bytes());
    push_len_prefixed(&mut payload, envelope.scope.as_bytes());
    push_len_prefixed(&mut payload, envelope.payload_hash_hex.as_bytes());
    match &envelope.proof {
        Some(proof) => {
            payload.push(1);
            push_len_prefixed(&mut payload, proof.nonce_hex.as_bytes());
            payload.extend_from_slice(&proof.expires_unix.to_be_bytes());
            push_len_prefixed(&mut payload, proof.proof_hex.as_bytes());
        }
        None => payload.push(0),
    }
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
    fn privilege_proof_round_trip() {
        let key = derive_super_key("secret", b"client-salt");
        let id = Uuid::new_v4();
        let hash = payload_hash(b"reboot");
        let nonce = [9_u8; 16];
        let proof = sign_privilege_proof(&key, id, "client:one", &hash, &nonce, 200);

        assert!(verify_privilege_proof(
            &key,
            id,
            "client:one",
            &hash,
            &proof,
            100
        ));
        assert!(!verify_privilege_proof(
            &key,
            id,
            "client:two",
            &hash,
            &proof,
            100
        ));
        assert!(!verify_privilege_proof(
            &key,
            id,
            "client:one",
            &hash,
            &proof,
            201
        ));
    }

    #[test]
    fn command_envelope_authorizes_once() {
        let key = derive_super_key("secret", b"client-salt");
        let signing = SigningKey::from_bytes(&[7_u8; 32]);
        let verifying = signing.verifying_key();
        let payload = br#"{"argv":["/bin/true"]}"#;
        let id = Uuid::new_v4();
        let hash = payload_hash(payload);
        let nonce = [3_u8; 16];
        let proof = sign_privilege_proof(&key, id, "client:lax-edge-01", &hash, &nonce, 200);
        let mut envelope = CommandEnvelope {
            command_id: id,
            scope: "client:lax-edge-01".to_string(),
            payload_hash_hex: hash,
            proof: Some(proof),
            server_signature: Vec::new(),
        };
        envelope.server_signature = sign_command_envelope(&signing, &envelope);

        let mut replay_cache = PrivilegeReplayCache::default();
        assert_eq!(
            verify_command_envelope(
                &key,
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
                &key,
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
    fn discovery_document_signature_rejects_tampering() {
        let signing = SigningKey::from_bytes(&[13_u8; 32]);
        let mut document = DiscoveryDocument {
            version: 1,
            issued_unix: 100,
            expires_unix: 160,
            endpoints: vec![crate::ServerEndpoint {
                label: "primary".to_string(),
                tcp_addr: "198.51.100.10:9443".to_string(),
                priority: 10,
            }],
            signature: Vec::new(),
        };
        document.signature = sign_discovery_document(&signing, &document);

        assert!(verify_discovery_document_signature(
            &signing.verifying_key(),
            &document
        ));

        document.endpoints[0].tcp_addr = "203.0.113.20:9443".to_string();
        assert!(!verify_discovery_document_signature(
            &signing.verifying_key(),
            &document
        ));
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
        let key = derive_super_key("secret", b"client-salt");
        let signing = SigningKey::from_bytes(&[9_u8; 32]);
        let verifying = signing.verifying_key();
        let payload = b"payload";
        let id = Uuid::new_v4();
        let hash = payload_hash(payload);
        let proof = sign_privilege_proof(&key, id, "client:one", &hash, &[4_u8; 16], 200);
        let mut envelope = CommandEnvelope {
            command_id: id,
            scope: "client:one".to_string(),
            payload_hash_hex: hash,
            proof: Some(proof),
            server_signature: Vec::new(),
        };
        envelope.server_signature = sign_command_envelope(&signing, &envelope);

        let mut replay_cache = PrivilegeReplayCache::default();
        assert_eq!(
            verify_command_envelope(
                &key,
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
                &key,
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
                &key,
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
    fn command_envelope_rejects_missing_or_expired_proof() {
        let key = derive_super_key("secret", b"client-salt");
        let signing = SigningKey::from_bytes(&[11_u8; 32]);
        let verifying = signing.verifying_key();
        let payload = b"payload";
        let id = Uuid::new_v4();
        let mut missing_proof = CommandEnvelope {
            command_id: id,
            scope: "client:one".to_string(),
            payload_hash_hex: payload_hash(payload),
            proof: None,
            server_signature: Vec::new(),
        };
        missing_proof.server_signature = sign_command_envelope(&signing, &missing_proof);

        let mut replay_cache = PrivilegeReplayCache::default();
        assert_eq!(
            verify_command_envelope(
                &key,
                &verifying,
                "client:one",
                payload,
                &missing_proof,
                100,
                &mut replay_cache,
            ),
            Err(AuthorizationError::MissingProof)
        );

        let expired_hash = payload_hash(payload);
        let expired_proof =
            sign_privilege_proof(&key, id, "client:one", &expired_hash, &[5_u8; 16], 99);
        let mut expired = CommandEnvelope {
            command_id: id,
            scope: "client:one".to_string(),
            payload_hash_hex: expired_hash,
            proof: Some(expired_proof),
            server_signature: Vec::new(),
        };
        expired.server_signature = sign_command_envelope(&signing, &expired);
        assert_eq!(
            verify_command_envelope(
                &key,
                &verifying,
                "client:one",
                payload,
                &expired,
                100,
                &mut replay_cache,
            ),
            Err(AuthorizationError::InvalidProof)
        );
    }
}
