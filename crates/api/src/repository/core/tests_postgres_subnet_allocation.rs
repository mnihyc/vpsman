use super::*;

fn subnet_input(
    name: &str,
    ipv4: (&str, &str, u8),
    ipv6: Option<(&str, &str, u8)>,
) -> TunnelPlanInput {
    let mut input =
        crate::tests_network::test_plan_input(RuntimeTunnelManager::AgentBuiltin, false);
    input.name = name.into();
    input.interface_name = name.into();
    input.ipv4_tunnel = Some(TunnelAddressPair {
        left: ipv4.0.into(),
        right: ipv4.1.into(),
        prefix_len: ipv4.2,
    });
    input.ipv6_tunnel = ipv6.map(|(left, right, prefix_len)| TunnelAddressPair {
        left: left.into(),
        right: right.into(),
        prefix_len,
    });
    input
}

async fn allocate(
    state: &crate::state::AppState,
    headers: &HeaderMap,
    request: serde_json::Value,
) -> Result<crate::model::AllocateTunnelEndpointsResponse, crate::error::ApiError> {
    crate::routes_network::allocate_tunnel_endpoints(
        State(state.clone()),
        headers.clone(),
        Json(serde_json::from_value(request).unwrap()),
    )
    .await
    .map(|response| response.0)
}

#[tokio::test]
async fn postgres_subnet_allocation_preserves_masks_current_pairs_and_clear_intent() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    insert_client(&db.pool, "client-a", None).await;
    insert_client(&db.pool, "client-b", None).await;
    let (operator, headers) = postgres_operator_session(&db.repo, "subnet-operator").await;
    let state = postgres_app_state(&db);
    let mut input = subnet_input(
        "manual",
        ("10.70.0.10", "10.70.0.20", 24),
        Some(("fd70::10", "fd70::20", 64)),
    );
    input.additional_addresses.left.ipv4 = vec!["10.70.0.30/24".into()];
    input.additional_addresses.left.ipv6 = vec!["fe80::10/64".into()];
    let stored = db
        .repo
        .record_tunnel_plan(&input, &plan_tunnel(&input).unwrap(), false, &operator)
        .await
        .unwrap();
    let fresh = allocate(
        &state,
        &headers,
        serde_json::json!({
            "ipv4_pool_cidr": "10.70.0.0/16", "ipv6_pool_cidr": "fd70::/48",
            "include_ipv4": true, "include_ipv6": true,
            "ipv4_prefix_len": 28, "ipv6_prefix_len": 80,
        }),
    )
    .await
    .unwrap();
    let fresh4 = fresh.ipv4_tunnel.unwrap();
    let fresh6 = fresh.ipv6_tunnel.unwrap();
    assert_eq!(
        (&*fresh4.left, &*fresh4.right, fresh4.prefix_len),
        ("10.70.1.1", "10.70.1.2", 28)
    );
    assert_eq!(
        (&*fresh6.left, &*fresh6.right, fresh6.prefix_len),
        ("fd70:0:0:1::", "fd70:0:0:1::1", 80)
    );
    // Editing retains pairs outside the pool and same-plan overlapping aliases.
    let retained = allocate(
        &state,
        &headers,
        serde_json::json!({
            "plan_id": stored.id,
            "include_ipv4": true, "include_ipv6": true,
            "ipv4_pool_cidr": "10.99.0.0/24", "ipv6_pool_cidr": "fd99::/64",
            "ipv4_prefix_len": 24, "ipv6_prefix_len": 64,
            "preferred_ipv4_tunnel": input.ipv4_tunnel,
            "preferred_ipv6_tunnel": input.ipv6_tunnel,
        }),
    )
    .await
    .unwrap();
    assert_eq!(retained.ipv4_tunnel, input.ipv4_tunnel);
    assert_eq!(retained.ipv6_tunnel, input.ipv6_tunnel);
    let retained_without_pools = allocate(
        &state,
        &headers,
        serde_json::json!({
            "plan_id": stored.id, "include_ipv4": true, "include_ipv6": false,
            "ipv4_prefix_len": 24, "preferred_ipv4_tunnel": input.ipv4_tunnel,
        }),
    )
    .await
    .unwrap();
    assert_eq!(retained_without_pools.ipv4_tunnel, input.ipv4_tunnel);
    // Each populated field is an anchor, not a requirement to populate its peer.
    // Neither family needs a pool when the subnet is already specified.
    for (left4, right4, expected4, left6, right6, expected6) in [
        (
            "192.0.2.10",
            "",
            ("192.0.2.10", "192.0.2.1"),
            "fd80::5",
            "",
            ("fd80::5", "fd80::"),
        ),
        (
            "",
            "192.0.2.20",
            ("192.0.2.1", "192.0.2.20"),
            "",
            "fd80::a",
            ("fd80::", "fd80::a"),
        ),
    ] {
        let anchored = allocate(
            &state,
            &headers,
            serde_json::json!({
                "include_ipv4": true, "include_ipv6": true,
                "ipv4_prefix_len": 24, "ipv6_prefix_len": 124,
                "preferred_ipv4_tunnel": {"left":left4, "right":right4, "prefix_len":24},
                "preferred_ipv6_tunnel": {"left":left6, "right":right6, "prefix_len":124},
            }),
        )
        .await
        .unwrap();
        let pair4 = anchored.ipv4_tunnel.unwrap();
        let pair6 = anchored.ipv6_tunnel.unwrap();
        assert_eq!(
            (&*pair4.left, &*pair4.right, pair4.prefix_len),
            (expected4.0, expected4.1, 24)
        );
        assert_eq!(
            (&*pair6.left, &*pair6.right, pair6.prefix_len),
            (expected6.0, expected6.1, 124)
        );
    }
    let blank_without_pool = allocate(
        &state,
        &headers,
        serde_json::json!({
            "include_ipv4": true, "include_ipv6": false, "ipv4_prefix_len": 24,
            "preferred_ipv4_tunnel": {"left":" ", "right":"", "prefix_len":24},
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(blank_without_pool.code, "ipv4_allocation_pool_required");
    let cleared = allocate(
        &state,
        &headers,
        serde_json::json!({
            "plan_id": stored.id, "include_ipv4": true, "include_ipv6": false,
            "ipv4_pool_cidr": "10.99.0.0/24", "ipv4_prefix_len": 30,
            "preferred_ipv4_tunnel": {"left":"", "right":" ", "prefix_len":30},
        }),
    )
    .await
    .unwrap()
    .ipv4_tunnel
    .unwrap();
    assert_eq!(
        (&*cleared.left, &*cleared.right, cleared.prefix_len),
        ("10.99.0.1", "10.99.0.2", 30)
    );
    assert_eq!(
        db.repo
            .get_tunnel_plan(stored.id)
            .await
            .unwrap()
            .unwrap()
            .revision,
        stored.revision,
        "allocation is a read-only proposal"
    );
    let external_overlap = allocate(
        &state,
        &headers,
        serde_json::json!({
            "include_ipv4": true, "include_ipv6": false, "ipv4_prefix_len": 28,
            "preferred_ipv4_tunnel": {"left":"10.70.0.65", "right":"10.70.0.66", "prefix_len":28},
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(external_overlap.code, "tunnel_plan_address_conflict");
    let missing = allocate(
        &state,
        &headers,
        serde_json::json!({
            "plan_id": Uuid::new_v4(), "include_ipv4": false, "include_ipv6": false,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_subnet_allocation_and_save_leave_additional_addresses_operator_owned() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    for client in ["client-a", "client-b", "client-c", "client-d"] {
        insert_client(&db.pool, client, None).await;
    }
    let (operator, headers) = postgres_operator_session(&db.repo, "subnet-alias-operator").await;
    let mut first = subnet_input(
        "alias-one",
        ("10.73.0.1", "10.73.0.2", 24),
        Some(("fd73::1", "fd73::2", 64)),
    );
    first.additional_addresses.left.ipv4 = vec!["10.73.1.30/24".into(), "10.74.0.10/24".into()];
    first.additional_addresses.left.ipv6 = vec!["fd74::30/64".into()];
    let stored = db
        .repo
        .record_tunnel_plan(&first, &plan_tunnel(&first).unwrap(), false, &operator)
        .await
        .unwrap();
    // Additional ranges may overlap another plan's main range or aliases. They
    // do not acquire allocation ownership merely by being configured on a link.
    let mut second = subnet_input(
        "alias-two",
        ("10.73.1.1", "10.73.1.2", 24),
        Some(("fd74::1", "fd74::2", 64)),
    );
    second.left_client_id = "client-c".into();
    second.right_client_id = "client-d".into();
    second.additional_addresses.left.ipv4 = vec!["10.73.0.30/24".into(), "10.74.0.10/24".into()];
    second.additional_addresses.left.ipv6 = vec!["fd73::30/64".into()];
    db.repo
        .record_tunnel_plan(&second, &plan_tunnel(&second).unwrap(), false, &operator)
        .await
        .unwrap();
    first.additional_addresses.right.ipv4 = vec!["10.73.1.40/24".into()];
    let changed = db
        .repo
        .update_tunnel_plan(
            stored.id,
            stored.revision,
            &first,
            &plan_tunnel(&first).unwrap(),
            false,
            &operator,
        )
        .await
        .unwrap();
    assert_eq!(
        changed.plan.additional_addresses,
        first.additional_addresses
    );
    let fresh = allocate(
        &postgres_app_state(&db),
        &headers,
        serde_json::json!({
            "include_ipv4":true, "include_ipv6":false,
            "ipv4_pool_cidr":"10.74.0.0/24", "ipv4_prefix_len":24,
        }),
    )
    .await
    .unwrap()
    .ipv4_tunnel
    .unwrap();
    assert_eq!(
        (&*fresh.left, &*fresh.right, fresh.prefix_len),
        ("10.74.0.1", "10.74.0.2", 24)
    );
    assert_eq!(
        db.repo
            .tunnel_plan_reserved_networks(None)
            .await
            .unwrap()
            .len(),
        4
    );
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_subnet_save_checks_overlap_globally_and_excludes_only_self() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    for client in ["client-a", "client-b", "client-c", "client-d"] {
        insert_client(&db.pool, client, None).await;
    }
    let (operator, headers) = postgres_operator_session(&db.repo, "subnet-save-operator").await;
    let mut first = subnet_input(
        "subnet-one",
        ("10.71.0.1", "10.71.0.2", 24),
        Some(("fd71::1", "fd71::2", 64)),
    );
    first.additional_addresses.left.ipv6 = vec!["fe80::10/64".into()];
    let stored = db
        .repo
        .record_tunnel_plan(&first, &plan_tunnel(&first).unwrap(), false, &operator)
        .await
        .unwrap();
    let mut second = subnet_input("subnet-two", ("10.71.0.65", "10.71.0.66", 28), None);
    second.left_client_id = "client-c".into();
    second.right_client_id = "client-d".into();
    assert_eq!(
        db.repo
            .record_tunnel_plan(&second, &plan_tunnel(&second).unwrap(), false, &operator)
            .await
            .unwrap_err()
            .to_string(),
        "tunnel_plan_address_conflict"
    );
    second.ipv4_tunnel = Some(TunnelAddressPair {
        left: "10.71.1.1".into(),
        right: "10.71.1.2".into(),
        prefix_len: 24,
    });
    second.ipv6_tunnel = Some(TunnelAddressPair {
        left: "fd71::100".into(),
        right: "fd71::101".into(),
        prefix_len: 80,
    });
    assert_eq!(
        db.repo
            .record_tunnel_plan(&second, &plan_tunnel(&second).unwrap(), false, &operator)
            .await
            .unwrap_err()
            .to_string(),
        "tunnel_plan_address_conflict"
    );
    second.ipv6_tunnel = Some(TunnelAddressPair {
        left: "fd71:0:0:1::1".into(),
        right: "fd71:0:0:1::2".into(),
        prefix_len: 64,
    });
    // IPv6 LL reuse remains legal on a different interface.
    second.additional_addresses.left.ipv6 = vec!["fe80::10/64".into()];
    let second_stored = db
        .repo
        .record_tunnel_plan(&second, &plan_tunnel(&second).unwrap(), false, &operator)
        .await
        .unwrap();
    first.ipv4_tunnel.as_mut().unwrap().left = "10.71.0.10".into();
    let changed = db
        .repo
        .update_tunnel_plan(
            stored.id,
            stored.revision,
            &first,
            &plan_tunnel(&first).unwrap(),
            false,
            &operator,
        )
        .await
        .unwrap();
    assert_eq!(changed.plan.ipv4_tunnel, first.ipv4_tunnel);
    let mut conflicting = second.clone();
    conflicting.ipv4_tunnel = Some(TunnelAddressPair {
        left: "10.71.0.33".into(),
        right: "10.71.0.34".into(),
        prefix_len: 28,
    });
    assert_eq!(
        db.repo
            .update_tunnel_plan(
                second_stored.id,
                second_stored.revision,
                &conflicting,
                &plan_tunnel(&conflicting).unwrap(),
                false,
                &operator
            )
            .await
            .unwrap_err()
            .to_string(),
        "tunnel_plan_address_conflict"
    );
    assert_eq!(
        db.repo
            .get_tunnel_plan(second_stored.id)
            .await
            .unwrap()
            .unwrap()
            .revision,
        second_stored.revision
    );
    let error = allocate(
        &postgres_app_state(&db),
        &headers,
        serde_json::json!({
            "plan_id": second_stored.id, "include_ipv4":true, "include_ipv6":false,
            "ipv4_prefix_len":28, "preferred_ipv4_tunnel": conflicting.ipv4_tunnel,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, "tunnel_plan_address_conflict");
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_subnet_concurrent_distinct_endpoints_cannot_save_overlapping_networks() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    for client in ["client-a", "client-b", "client-c", "client-d"] {
        insert_client(&db.pool, client, None).await;
    }
    let operator = postgres_network_operator(&db.repo).await;
    // Disjoint clients, interfaces and IPs isolate the shared subnet lock.
    let first = subnet_input("race-one", ("10.72.0.1", "10.72.0.2", 24), None);
    let mut second = subnet_input("race-two", ("10.72.0.65", "10.72.0.66", 28), None);
    second.left_client_id = "client-c".into();
    second.right_client_id = "client-d".into();
    let first_plan = plan_tunnel(&first).unwrap();
    let second_plan = plan_tunnel(&second).unwrap();
    let (left, right) = tokio::join!(
        db.repo
            .record_tunnel_plan(&first, &first_plan, false, &operator),
        db.repo
            .record_tunnel_plan(&second, &second_plan, false, &operator),
    );
    match (left, right) {
        (Ok(_), Err(error)) | (Err(error), Ok(_)) => {
            assert_eq!(error.to_string(), "tunnel_plan_address_conflict")
        }
        outcome => panic!("one overlapping save must fail: {outcome:?}"),
    }
    assert_eq!(db.repo.list_tunnel_plans().await.unwrap().len(), 1);
    db.cleanup().await;
}
