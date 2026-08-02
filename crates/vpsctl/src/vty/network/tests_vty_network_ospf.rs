use super::parse_vty_tunnel_ospf_cost_update;

const HASH_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const HASH_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

#[test]
fn parses_endpoint_adapter_snapshot() {
    let request = parse_vty_tunnel_ospf_cost_update(&[
        "--plan-id",
        "00000000-0000-0000-0000-000000000001",
        "--plan-revision=7",
        "--recommendation-id=ospf-1234abcd5678ef90",
        "--left-current-ospf-cost",
        "100",
        "--right-current-ospf-cost=90",
        "--desired-ospf-cost=50",
        "--left-adapter-definition-hash",
        HASH_A,
        "--right-adapter-definition-hash",
        HASH_B,
        "--confirmed",
    ])
    .unwrap();

    assert_eq!(
        request.plan_id.to_string(),
        "00000000-0000-0000-0000-000000000001"
    );
    assert_eq!(request.recommendation_id, "ospf-1234abcd5678ef90");
    assert_eq!(request.plan_revision, 7);
    assert_eq!(request.left_current_ospf_cost, Some(100));
    assert_eq!(request.right_current_ospf_cost, Some(90));
    assert_eq!(request.desired_ospf_cost, 50);
    assert_eq!(request.left_adapter_definition_hash, HASH_A);
    assert_eq!(request.right_adapter_definition_hash, HASH_B);
    assert!(request.confirmed);
}

#[test]
fn accepts_unknown_endpoint_cost_but_rejects_stale_or_incomplete_snapshots() {
    assert!(parse_vty_tunnel_ospf_cost_update(&[
        "--plan-id=00000000-0000-0000-0000-000000000001",
        "--plan-revision=7",
        "--recommendation-id=ospf-1234abcd5678ef90",
        "--right-current-ospf-cost=90",
        "--desired-ospf-cost=50",
        "--left-adapter-definition-hash",
        HASH_A,
        "--right-adapter-definition-hash",
        HASH_B,
        "--confirmed",
    ])
    .is_ok());
    assert!(parse_vty_tunnel_ospf_cost_update(&[
        "--plan-id=00000000-0000-0000-0000-000000000001",
        "--plan-revision=7",
        "--recommendation-id=ospf-1234abcd5678ef90",
        "--left-current-ospf-cost=50",
        "--right-current-ospf-cost=50",
        "--desired-ospf-cost=50",
        "--left-adapter-definition-hash",
        HASH_A,
        "--right-adapter-definition-hash",
        HASH_B,
        "--confirmed",
    ])
    .is_err());
    assert!(parse_vty_tunnel_ospf_cost_update(&[
        "--plan-id=00000000-0000-0000-0000-000000000001",
        "--plan-revision=7",
        "--recommendation-id=ospf-1234abcd5678ef90",
        "--desired-ospf-cost=50",
        "--left-adapter-definition-hash=not-a-hash",
        "--right-adapter-definition-hash",
        HASH_B,
        "--confirmed",
    ])
    .is_err());
}
