use super::*;

async fn ping_display_fixture(db: &PgReliabilityTestDb, client_id: &str) -> [Uuid; 3] {
    insert_client(&db.pool, client_id, None).await;
    let ids = [
        "00000000-0000-4000-8000-000000000001",
        "00000000-0000-4000-8000-000000000002",
        "00000000-0000-4000-8000-000000000003",
    ]
    .map(|id| Uuid::parse_str(id).unwrap());
    for (index, id) in ids.iter().enumerate() {
        sqlx::query("INSERT INTO ping_targets (id,name,host,probe_kind,enabled,generation) VALUES ($1,$2,'192.0.2.1','icmp',$3,7)")
            .bind(id).bind(["Alpha", "Bravo", "Charlie"][index]).bind(index != 2)
            .execute(&db.pool).await.unwrap();
        sqlx::query("INSERT INTO ping_target_assignments (target_id,client_id,is_primary) VALUES ($1,$2,$3)")
            .bind(id).bind(client_id).bind(index == 0).execute(&db.pool).await.unwrap();
    }
    ids
}

async fn ping_display_http(
    router: &axum::Router,
    headers: &HeaderMap,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut request = Request::builder().method(method).uri(uri);
    for (name, value) in headers {
        request = request.header(name, value);
    }
    let body = body.map_or_else(Body::empty, |body| Body::from(body.to_string()));
    let response = router
        .clone()
        .oneshot(
            request
                .header(CONTENT_TYPE, "application/json")
                .body(body)
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or_else(|_| json!(String::from_utf8_lossy(&bytes))),
    )
}

async fn ping_display_runtime_snapshot(pool: &PgPool) -> Value {
    sqlx::query_scalar(
        r#"
        SELECT jsonb_build_object(
            'targets', (SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM ping_targets t),
            'assignments', (SELECT jsonb_agg(to_jsonb(a) ORDER BY target_id,client_id) FROM ping_target_assignments a),
            'runtime_state', (SELECT jsonb_agg(to_jsonb(s) ORDER BY client_id) FROM client_runtime_config_apply_state s),
            'runtime_owners', (SELECT jsonb_agg(to_jsonb(o)) FROM client_runtime_config_owners o),
            'reconcile', (SELECT jsonb_agg(to_jsonb(w)) FROM client_runtime_config_reconcile_work w),
            'jobs', (SELECT jsonb_agg(to_jsonb(j) ORDER BY id) FROM jobs j)
        )
        "#,
    ).fetch_one(pool).await.unwrap()
}

#[tokio::test]
async fn postgres_ping_display_reorder_is_persistent_atomic_and_runtime_independent() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client_id = "ping-display-edge";
    let ids = ping_display_fixture(&db, client_id).await;
    let (operator, headers) = postgres_operator_session(&db.repo, "ping-display-operator").await;
    let router = crate::routes::build_router(postgres_app_state(&db));
    let initial = db.repo.list_ping_targets().await.unwrap();
    assert_eq!(
        initial.iter().map(|target| target.id).collect::<Vec<_>>(),
        ids
    );
    assert!(initial.iter().all(|target| target.display_order.is_none()));
    assert_eq!(
        initial
            .iter()
            .map(|target| target.display_color.as_str())
            .collect::<Vec<_>>(),
        ["#129eaf", "#5f6368", "#8a4d00"]
    );
    let runtime_before = ping_display_runtime_snapshot(&db.pool).await;
    let wire_before =
        serde_json::to_value(db.repo.ping_targets_for_client(client_id).await.unwrap()).unwrap();
    let put = "/api/v1/ping-targets/display";
    let (status, saved) = ping_display_http(
        &router,
        &headers,
        "PUT",
        put,
        Some(json!({"targets":[
            {"target_id":ids[2],"color":" #AbC "}, {"target_id":ids[0],"color":"#123456"}
        ]})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(
        saved,
        json!([
            {"target_id":ids[2],"display_order":0,"display_color":"#aabbcc"},
            {"target_id":ids[0],"display_order":1,"display_color":"#123456"},
            {"target_id":ids[1],"display_order":2,"display_color":"#5f6368"}
        ])
    );
    let (status, saved) = ping_display_http(
        &router,
        &headers,
        "PUT",
        put,
        Some(json!({"targets":[
            {"target_id":ids[1],"color":"#DEF"}
        ]})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(
        saved,
        json!([
            {"target_id":ids[1],"display_order":0,"display_color":"#ddeeff"},
            {"target_id":ids[2],"display_order":1,"display_color":"#aabbcc"},
            {"target_id":ids[0],"display_order":2,"display_color":"#123456"}
        ])
    );
    let (_, empty_save) =
        ping_display_http(&router, &headers, "PUT", put, Some(json!({"targets":[]}))).await;
    assert_eq!(
        empty_save, saved,
        "omitting all targets preserves the existing display"
    );
    let reloaded = db.repo.list_ping_targets().await.unwrap();
    assert_eq!(
        reloaded.iter().map(|target| target.id).collect::<Vec<_>>(),
        [ids[1], ids[2], ids[0]]
    );
    for target in reloaded {
        let detail = db
            .repo
            .get_ping_target_detail(target.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(detail.target.display_order, target.display_order);
        assert_eq!(detail.target.display_color, target.display_color);
    }
    let current = db
        .repo
        .current_ping_targets_for_client(client_id)
        .await
        .unwrap();
    assert_eq!(
        current
            .iter()
            .map(|target| target.target_id)
            .collect::<Vec<_>>(),
        [ids[1], ids[2], ids[0]]
    );
    assert_eq!(
        current[1].state, "disabled",
        "display includes disabled definitions"
    );
    let primary = db
        .repo
        .current_primary_ping_for_clients(&[client_id.to_string()])
        .await
        .unwrap();
    assert_eq!(
        primary[0].1.target_id, ids[0],
        "display order never changes the primary target"
    );
    assert_eq!(primary[0].1.display_color, "#123456");
    for (body, code) in [
        (
            json!({"targets":[{"target_id":ids[0],"color":"#000"},{"target_id":ids[0],"color":"#fff"}]}),
            "ping_target_display_duplicate",
        ),
        (
            json!({"targets":[{"target_id":Uuid::nil(),"color":"#000"}]}),
            "ping_target_display_unknown",
        ),
        (
            json!({"targets":[{"target_id":ids[0],"color":"red"}]}),
            "ping_target_display_color_invalid",
        ),
    ] {
        let (status, error) = ping_display_http(&router, &headers, "PUT", put, Some(body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
        assert!(error.to_string().contains(code), "{error}");
    }
    let persisted: Value = sqlx::query_scalar("SELECT jsonb_agg(jsonb_build_object('target_id',target_id,'display_order',display_order,'display_color',display_color) ORDER BY display_order) FROM ping_target_display")
        .fetch_one(&db.pool).await.unwrap();
    assert_eq!(
        persisted, saved,
        "invalid writes roll back without partial reorder"
    );
    let audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_logs WHERE action='ping_target.display_updated'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(audits, 3);
    let audit: Value = sqlx::query_scalar("SELECT metadata FROM audit_logs WHERE action='ping_target.display_updated' ORDER BY created_at DESC,id DESC LIMIT 1")
        .fetch_one(&db.pool).await.unwrap();
    assert_eq!(audit["result"], "succeeded");
    assert_eq!(audit["origin_kind"], "operator_request");
    assert_eq!(audit["component"], "monitoring-controller");
    assert_eq!(audit["operator_id"], operator.operator.id.to_string());
    assert_eq!(
        ping_display_runtime_snapshot(&db.pool).await,
        runtime_before
    );
    assert_eq!(
        serde_json::to_value(db.repo.ping_targets_for_client(client_id).await.unwrap()).unwrap(),
        wire_before
    );
    sqlx::query("UPDATE operators SET scopes='[\"network:read\"]'::jsonb WHERE id=$1")
        .bind(operator.operator.id)
        .execute(&db.pool)
        .await
        .unwrap();
    let (status, _) =
        ping_display_http(&router, &headers, "PUT", put, Some(json!({"targets":[]}))).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = ping_display_http(
        &router,
        &HeaderMap::new(),
        "PUT",
        put,
        Some(json!({"targets":[]})),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    sqlx::query("DELETE FROM ping_targets WHERE id=$1")
        .bind(ids[2])
        .execute(&db.pool)
        .await
        .unwrap();
    let remaining: i64 =
        sqlx::query_scalar("SELECT count(*) FROM ping_target_display WHERE target_id=$1")
            .bind(ids[2])
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(
        remaining, 0,
        "presentation ownership cascades with target deletion"
    );
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_ping_display_refreshes_private_public_metadata_without_public_ids() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client_id = "ping-display-share-edge";
    let ids = ping_display_fixture(&db, client_id).await;
    let (operator, headers) =
        postgres_operator_session(&db.repo, "ping-display-share-operator").await;
    let share = crate::model::MonitoringShareRecord {
        id: Uuid::new_v4(),
        name: "Ping display".to_string(),
        token_secret: "a".repeat(64),
        selector_expression: "*".to_string(),
        targets: vec![crate::model::MonitoringShareTargetRecord {
            client_id: client_id.to_string(),
            public_client_key: "0".repeat(64),
        }],
        visibility: crate::model::MonitoringShareVisibilityView {
            identity_context: false,
            billing: false,
            system_information: false,
            resources: false,
            network: false,
            traffic: false,
            ping: true,
            detail_history: true,
        },
        expires_at: (crate::unix_now() + 3600).to_string(),
        revoked_at: None,
        created_at: crate::unix_now().to_string(),
        updated_at: crate::unix_now().to_string(),
    };
    db.repo
        .create_monitoring_share(share.clone(), &operator)
        .await
        .unwrap();
    let (visitor_id, _) = db
        .repo
        .record_monitoring_share_visitor(&share, None, "192.0.2.1", None)
        .await
        .unwrap();
    let mut public_headers = HeaderMap::new();
    public_headers.insert("x-vpsman-share-token", share.token_secret.parse().unwrap());
    public_headers.insert(
        "x-vpsman-share-visitor",
        visitor_id.to_string().parse().unwrap(),
    );
    let router = crate::routes::build_router(postgres_app_state(&db));
    let private_uri = format!("/api/v1/clients/{client_id}/monitoring?projection=ping&window=15m");
    let public_uri = format!(
        "/api/v1/public/monitoring-shares/{}/data?client_key={}&window=15m",
        share.id,
        "0".repeat(64)
    );
    let (status, before) = ping_display_http(&router, &headers, "GET", &private_uri, None).await;
    assert_eq!(status, StatusCode::OK, "{before}");
    assert_eq!(before["ping_targets"][0]["display_color"], "#129eaf");
    let (status, before_public) =
        ping_display_http(&router, &public_headers, "GET", &public_uri, None).await;
    assert_eq!(status, StatusCode::OK, "{before_public}");
    assert_eq!(
        before_public["detail"]["ping_targets"][0]["display_color"],
        "#129eaf"
    );
    let (status, saved) = ping_display_http(
        &router,
        &headers,
        "PUT",
        "/api/v1/ping-targets/display",
        Some(json!({"targets":[
            {"target_id":ids[1],"color":"#00F"},{"target_id":ids[0],"color":"#F00"}
        ]})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    let (status, after) = ping_display_http(&router, &headers, "GET", &private_uri, None).await;
    assert_eq!(status, StatusCode::OK, "{after}");
    assert_eq!(after["ping_targets"][0]["target_id"], ids[1].to_string());
    assert_eq!(after["ping_targets"][0]["display_color"], "#0000ff");
    let (status, after_public) =
        ping_display_http(&router, &public_headers, "GET", &public_uri, None).await;
    assert_eq!(status, StatusCode::OK, "{after_public}");
    assert_eq!(
        after_public["detail"]["ping_targets"][0]["target_name"],
        "Bravo"
    );
    assert_eq!(
        after_public["detail"]["ping_targets"][0]["display_color"],
        "#0000ff"
    );
    assert_eq!(
        after_public["detail"]["ping_targets"][1]["display_order"],
        1
    );
    assert_eq!(
        after_public["cards"][0]["primary_ping"]["target_name"],
        "Alpha"
    );
    assert_eq!(
        after_public["cards"][0]["primary_ping"]["display_color"],
        "#ff0000"
    );
    for id in ids {
        assert!(
            !after_public.to_string().contains(&id.to_string()),
            "public presentation must not disclose internal target IDs"
        );
    }
    db.cleanup().await;
}
