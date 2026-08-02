use super::*;

#[test]
fn privilege_assertion_authorizes_once_for_exact_intent() {
    let verifier_key = [3_u8; 32];
    let intent = r#"{"action":"job.dispatch","target":"client-a"}"#;
    let intent_hash = payload_hash(intent.as_bytes());
    let assertion = sign_privilege_assertion(&verifier_key, &intent_hash, &[7_u8; 16], 100, 300);
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
fn privilege_assertion_replay_cache_fails_closed_at_its_hard_bound() {
    let verifier_key = [10_u8; 32];
    let intent = r#"{"action":"job.dispatch","target":"fleet"}"#;
    let intent_hash = payload_hash(intent.as_bytes());
    let mut replay_cache = PrivilegeAssertionReplayCache::new(2);

    for nonce in [[1_u8; 16], [2_u8; 16]] {
        let assertion = sign_privilege_assertion(&verifier_key, &intent_hash, &nonce, 100, 300);
        assert_eq!(
            verify_privilege_assertion(&verifier_key, intent, &assertion, 120, &mut replay_cache),
            Ok(intent_hash.clone())
        );
    }
    let third = sign_privilege_assertion(&verifier_key, &intent_hash, &[3_u8; 16], 100, 300);
    assert_eq!(
        verify_privilege_assertion(&verifier_key, intent, &third, 120, &mut replay_cache),
        Err(PrivilegeAssertionError::ReplayProtectionSaturated)
    );
    assert_eq!(replay_cache.seen.len(), 2);
    assert_eq!(replay_cache.order.len(), 2);

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
    let assertion = sign_privilege_assertion(&verifier_key, &intent_hash, &[8_u8; 16], 100, 300);
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
