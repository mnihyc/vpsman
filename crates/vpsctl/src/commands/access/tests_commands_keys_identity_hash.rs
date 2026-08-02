use super::agent_identity_request_payload_hash;

#[test]
fn agent_identity_request_hash_matches_the_shared_canonical_shape() {
    let tags = vec![" edge ".to_string(), "bgp".to_string(), "edge".to_string()];
    assert_eq!(
        agent_identity_request_payload_hash(
            " v-16 ",
            &"11".repeat(32),
            Some(" Edge 16 "),
            &tags,
            false,
        )
        .unwrap(),
        "fe02d0d023921dead3370b45a0c9e256464173ce30d4c6ee9d7ccc173f9a078c"
    );
}

#[test]
fn agent_identity_request_hash_rejects_invalid_public_keys() {
    assert!(agent_identity_request_payload_hash("v-1", "xyz", None, &[], false).is_err());
    assert!(agent_identity_request_payload_hash("v-1", "11", None, &[], false).is_err());
}
