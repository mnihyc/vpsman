use std::collections::{HashMap, VecDeque};

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
    seen: HashMap<String, u64>,
    order: VecDeque<(String, u64)>,
}

impl Default for PrivilegeAssertionReplayCache {
    fn default() -> Self {
        Self::new(4096)
    }
}

impl PrivilegeAssertionReplayCache {
    pub fn new(max_entries: usize) -> Self {
        Self {
            seen: HashMap::with_capacity(max_entries.max(1)),
            order: VecDeque::new(),
        }
    }

    pub fn remember(
        &mut self,
        nonce_hex: &str,
        expires_unix: u64,
        now_unix: u64,
    ) -> Result<(), PrivilegeAssertionError> {
        self.purge_expired(now_unix);
        let nonce_hex = nonce_hex.to_string();
        if self.seen.contains_key(&nonce_hex) {
            return Err(PrivilegeAssertionError::Replay);
        }
        self.seen.insert(nonce_hex.clone(), expires_unix);
        self.order.push_back((nonce_hex, expires_unix));
        Ok(())
    }

    fn purge_expired(&mut self, now_unix: u64) {
        let mut active = VecDeque::with_capacity(self.order.len());
        while let Some((nonce_hex, expires_unix)) = self.order.pop_front() {
            if expires_unix < now_unix {
                if self
                    .seen
                    .get(&nonce_hex)
                    .is_some_and(|current_expires| *current_expires == expires_unix)
                {
                    self.seen.remove(&nonce_hex);
                }
            } else {
                active.push_back((nonce_hex, expires_unix));
            }
        }
        self.order = active;
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
    let replay_expires_unix = assertion
        .issued_unix
        .saturating_add(MAX_PRIVILEGE_ASSERTION_AGE_SECS)
        .min(assertion.expires_unix);
    replay_cache.remember(&assertion.nonce_hex, replay_expires_unix, now_unix)?;
    Ok(intent_hash_hex)
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
    fn privilege_assertion_replay_cache_keeps_unexpired_nonces_under_churn() {
        let verifier_key = [10_u8; 32];
        let intent = r#"{"action":"job.dispatch","target":"fleet"}"#;
        let intent_hash = payload_hash(intent.as_bytes());
        let mut replay_cache = PrivilegeAssertionReplayCache::new(2);

        for nonce in [[1_u8; 16], [2_u8; 16], [3_u8; 16]] {
            let assertion = sign_privilege_assertion(&verifier_key, &intent_hash, &nonce, 100, 300);
            assert_eq!(
                verify_privilege_assertion(
                    &verifier_key,
                    intent,
                    &assertion,
                    120,
                    &mut replay_cache
                ),
                Ok(intent_hash.clone())
            );
        }

        let first = sign_privilege_assertion(&verifier_key, &intent_hash, &[1_u8; 16], 100, 300);
        assert_eq!(
            verify_privilege_assertion(&verifier_key, intent, &first, 121, &mut replay_cache),
            Err(PrivilegeAssertionError::Replay)
        );
    }

    #[test]
    fn privilege_assertion_replay_cache_purges_expired_nonces() {
        let mut replay_cache = PrivilegeAssertionReplayCache::new(1);

        replay_cache.remember("nonce-a", 10, 1).unwrap();
        assert_eq!(
            replay_cache.remember("nonce-a", 10, 10),
            Err(PrivilegeAssertionError::Replay)
        );
        assert_eq!(replay_cache.remember("nonce-a", 10, 11), Ok(()));
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
