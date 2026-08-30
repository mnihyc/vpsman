use super::*;
use crate::repository_key_lifecycle::agent_identity_payload_hash;

#[test]
fn identity_payload_hash_is_normalized_and_binds_every_material_field() {
    let base = UpsertAgentIdentityRequest {
        client_id: Some(" edge-a ".to_string()),
        client_public_key_hex: "11".repeat(32),
        display_name: Some(" Edge A ".to_string()),
        tags: vec!["region:sg".to_string(), " role:web ".to_string()],
        replace_existing_key: false,
        confirmed: true,
        privilege_assertion: None,
    };
    let hash = agent_identity_payload_hash(&base).unwrap();
    assert_eq!(
        hash,
        "f00a5fe086d6dc7f7ec8081a7f2d7d6cc440805fcd4fdf6b32aa96fa61ce95c6"
    );
    let mut normalized = UpsertAgentIdentityRequest {
        client_id: Some("edge-a".to_string()),
        client_public_key_hex: "11".repeat(32),
        display_name: Some("Edge A".to_string()),
        tags: vec![
            "role:web".to_string(),
            "region:sg".to_string(),
            "role:web".to_string(),
        ],
        replace_existing_key: false,
        confirmed: true,
        privilege_assertion: None,
    };
    assert_eq!(hash, agent_identity_payload_hash(&normalized).unwrap());

    normalized.client_public_key_hex = "22".repeat(32);
    assert_ne!(hash, agent_identity_payload_hash(&normalized).unwrap());
    normalized.client_public_key_hex = "11".repeat(32);
    normalized.tags.push("tier:prod".to_string());
    assert_ne!(hash, agent_identity_payload_hash(&normalized).unwrap());
    normalized.tags.pop();
    normalized.replace_existing_key = true;
    assert_ne!(hash, agent_identity_payload_hash(&normalized).unwrap());
}
