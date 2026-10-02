use super::*;

async fn saved_plan(
    db: &PgReliabilityTestDb,
    name: &str,
    subnet: u8,
    enabled: bool,
    ospf: bool,
    operator: &AuthContext,
) -> crate::model::TunnelPlanView {
    let mut input = crate::tests_network::test_plan_input(RuntimeTunnelManager::AgentBuiltin, ospf);
    input.name = name.into();
    input.interface_name = name.into();
    // Distinct /31 links avoid the real repository's global address conflicts.
    input.address_pool_cidr = format!("10.10.{subnet}.0/29");
    input.ipv4_tunnel = Some(TunnelAddressPair {
        left: format!("10.10.{subnet}.0"),
        right: format!("10.10.{subnet}.1"),
        prefix_len: 31,
    });
    input.left_client_id = format!("{name}-left");
    input.right_client_id = format!("{name}-right");
    for client in [&input.left_client_id, &input.right_client_id] {
        insert_client(&db.pool, client, None).await;
    }
    crate::tests_network::seed_test_plan_adapter_definitions(&db.repo, &input).await;
    db.repo
        .record_tunnel_plan(&input, &plan_tunnel(&input).unwrap(), enabled, operator)
        .await
        .unwrap()
}

#[tokio::test]
async fn postgres_directional_ospf_targets_verify_independently_and_preserve_partial_failures() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let operator = postgres_network_operator(&db.repo).await;
    let saved = saved_plan(&db, "ospf-pair", 0, true, true, &operator).await;
    let mut current = (None, None);
    for (left_result, right_result, right_success, expected_status) in [
        (110, 45, true, "verified"),
        (45, 110, true, "stale"),
        (110, 45, false, "partial"),
    ] {
        let left_job = Uuid::new_v4();
        let right_job = Uuid::new_v4();
        let staged = db
            .repo
            .stage_tunnel_plan_ospf_jobs(
                saved.id,
                saved.revision,
                current.0,
                current.1,
                Some((110, 45)),
                left_job,
                right_job,
                &operator,
            )
            .await
            .unwrap();
        assert_eq!(
            staged.desired_ospf_cost, None,
            "legacy scalar cannot represent an asymmetric pair"
        );
        assert_eq!(
            (
                staged.left_desired_ospf_cost,
                staged.right_desired_ospf_cost
            ),
            (Some(110), Some(45))
        );
        assert!(db
            .repo
            .stage_tunnel_plan_ospf_jobs(
                saved.id,
                saved.revision,
                current.0,
                current.1,
                Some((110, 45)),
                Uuid::new_v4(),
                Uuid::new_v4(),
                &operator
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("job_in_progress"));
        let first = db
            .repo
            .record_tunnel_plan_ospf_job_result(
                saved.id,
                TunnelEndpointSide::Left,
                left_job,
                Some(left_result),
                true,
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(first.ospf_status, "pending");
        let completed = db
            .repo
            .record_tunnel_plan_ospf_job_result(
                saved.id,
                TunnelEndpointSide::Right,
                right_job,
                Some(right_result),
                right_success,
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(completed.ospf_status, expected_status);
        assert!(
            db.repo
                .record_tunnel_plan_ospf_job_result(
                    saved.id,
                    TunnelEndpointSide::Right,
                    right_job,
                    Some(999),
                    true
                )
                .await
                .unwrap()
                .is_none(),
            "duplicate results cannot overwrite a completed attempt"
        );
        current = (Some(left_result), Some(right_result));
    }
    assert!(db
        .repo
        .stage_tunnel_plan_ospf_jobs(
            saved.id,
            saved.revision + 1,
            current.0,
            current.1,
            Some((110, 45)),
            Uuid::new_v4(),
            Uuid::new_v4(),
            &operator
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("snapshot_stale"));
    let symmetric = db
        .repo
        .stage_tunnel_plan_ospf_jobs(
            saved.id,
            saved.revision,
            current.0,
            current.1,
            Some((50, 50)),
            Uuid::new_v4(),
            Uuid::new_v4(),
            &operator,
        )
        .await
        .unwrap();
    assert_eq!(symmetric.desired_ospf_cost, Some(50));
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_directional_ospf_migration_preserves_pending_targets_history_and_disabled_plans()
{
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let operator = postgres_network_operator(&db.repo).await;
    let active = saved_plan(&db, "ospf-active", 0, true, true, &operator).await;
    let disabled = saved_plan(&db, "ospf-off", 1, false, true, &operator).await;
    let no_ospf = saved_plan(&db, "ospf-none", 2, true, false, &operator).await;
    let left_job = Uuid::new_v4();
    let right_job = Uuid::new_v4();
    // Reconstruct the v0.5.34 columns and JSON before running the actual migration.
    sqlx::raw_sql("ALTER TABLE tunnel_plans DROP COLUMN left_desired_ospf_cost, DROP COLUMN right_desired_ospf_cost").execute(&db.pool).await.unwrap();
    sqlx::query("UPDATE tunnel_plans SET input=jsonb_set(input,'{ospf}',(input->'ospf') - ARRAY['left_cost_offset','right_cost_offset','left_cost_multiplier','right_cost_multiplier','cost_floor']), plan=jsonb_set(plan - ARRAY['left_recommended_ospf_cost','right_recommended_ospf_cost'],'{ospf}',(plan->'ospf') - ARRAY['left_cost_offset','right_cost_offset','left_cost_multiplier','right_cost_multiplier','cost_floor']) WHERE id=ANY($1)")
        .bind(vec![active.id, disabled.id]).execute(&db.pool).await.unwrap();
    sqlx::query("UPDATE tunnel_plans SET desired_ospf_cost=49, ospf_status='pending', left_ospf_status='pending', right_ospf_status='pending', left_current_ospf_cost=54, right_current_ospf_cost=49, left_ospf_job_id=$2, right_ospf_job_id=$3 WHERE id=$1")
        .bind(active.id).bind(left_job).bind(right_job).execute(&db.pool).await.unwrap();
    // An off-grid lower bound must still win over the shared floor.
    sqlx::query("UPDATE tunnel_plans SET plan=jsonb_set(jsonb_set(plan,'{recommended_ospf_cost}','49'),'{ospf,policy,min_cost}','47'), input=jsonb_set(input,'{ospf,policy,min_cost}','47'), recommended_ospf_cost=49 WHERE id=$1")
        .bind(disabled.id).execute(&db.pool).await.unwrap();
    let old_plan: Value = sqlx::query_scalar("SELECT plan FROM tunnel_plans WHERE id=$1")
        .bind(active.id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let old_topology = tunnel_topology_identity_hash(
        active.id,
        &serde_json::from_value::<vpsman_common::TunnelPlan>(old_plan).unwrap(),
    );
    let series_id: i64 = sqlx::query_scalar("INSERT INTO network_observation_series(plan_id, topology_identity_hash, plan_name, interface_name, client_id, peer_client_id, endpoint_side, address_family, target) VALUES($1,$2,$3,$4,$5,$6,'left','ipv4','10.10.0.1') RETURNING id")
        .bind(active.id).bind(&old_topology).bind(&active.name).bind(&active.plan.interface_name).bind(&active.left_client_id).bind(&active.right_client_id).fetch_one(&db.pool).await.unwrap();
    let before: Value = sqlx::query_scalar("SELECT to_jsonb(t) - ARRAY['input','plan','revision','operational_alert_runtime_boundary_at'] FROM tunnel_plans t WHERE id=$1").bind(active.id).fetch_one(&db.pool).await.unwrap();
    let off_before: Value =
        sqlx::query_scalar("SELECT to_jsonb(t) FROM tunnel_plans t WHERE id=$1")
            .bind(no_ospf.id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    let runtime_before: i64 = sqlx::query_scalar(
        "SELECT desired_revision FROM client_runtime_config_reconcile_work WHERE client_id=$1",
    )
    .bind(&active.left_client_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let boundary_before: String = sqlx::query_scalar(
        "SELECT operational_alert_runtime_boundary_at::text FROM tunnel_plans WHERE id=$1",
    )
    .bind(active.id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    sqlx::raw_sql(include_str!(
        "../../../../../migrations/0028_directional_ospf_costs.sql"
    ))
    .execute(&db.pool)
    .await
    .unwrap();
    let runtime_after: i64 = sqlx::query_scalar(
        "SELECT desired_revision FROM client_runtime_config_reconcile_work WHERE client_id=$1",
    )
    .bind(&active.left_client_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert!(
        runtime_after > runtime_before,
        "normal configuration triggers publish the new evidence identity"
    );
    let boundary_after: String = sqlx::query_scalar(
        "SELECT operational_alert_runtime_boundary_at::text FROM tunnel_plans WHERE id=$1",
    )
    .bind(active.id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_ne!(
        boundary_before, boundary_after,
        "old runtime evidence cannot certify the new plan snapshot"
    );
    let after: Value = sqlx::query_scalar("SELECT to_jsonb(t) - ARRAY['input','plan','revision','operational_alert_runtime_boundary_at','left_desired_ospf_cost','right_desired_ospf_cost'] FROM tunnel_plans t WHERE id=$1").bind(active.id).fetch_one(&db.pool).await.unwrap();
    assert_eq!(
        before, after,
        "pending jobs, reported costs and timestamps are untouched"
    );
    let off_after: Value = sqlx::query_scalar("SELECT to_jsonb(t) - ARRAY['left_desired_ospf_cost','right_desired_ospf_cost'] FROM tunnel_plans t WHERE id=$1").bind(no_ospf.id).fetch_one(&db.pool).await.unwrap();
    assert_eq!(off_before, off_after);
    let migrated = db
        .repo
        .get_tunnel_plan_record(active.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(migrated.revision, active.revision + 1);
    assert_eq!(
        (
            migrated.left_desired_ospf_cost,
            migrated.right_desired_ospf_cost
        ),
        (Some(49), Some(49))
    );
    assert_eq!(migrated.ospf_status, "pending");
    assert_eq!(migrated.input.ospf.as_ref().unwrap().cost_floor, 5);
    assert_eq!(migrated.plan.ospf.as_ref().unwrap().left_cost_offset, 0.0);
    assert_eq!(
        migrated.plan.ospf.as_ref().unwrap().right_cost_multiplier,
        1.0
    );
    assert_eq!(
        tunnel_topology_identity_hash(active.id, &migrated.plan),
        old_topology
    );
    let persisted_plan: Value = sqlx::query_scalar("SELECT plan FROM tunnel_plans WHERE id=$1")
        .bind(active.id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let persisted_plan: vpsman_common::TunnelPlan = serde_json::from_value(persisted_plan).unwrap();
    assert_eq!(
        serde_json::to_value(persisted_plan).unwrap(),
        serde_json::to_value(&migrated.plan).unwrap(),
        "runtime evidence sees the same paired preview in every reader"
    );
    let disabled = db
        .repo
        .get_tunnel_plan_record(disabled.id)
        .await
        .unwrap()
        .unwrap();
    assert!(!disabled.enabled);
    assert_eq!(disabled.plan.left_recommended_ospf_cost, Some(47));
    assert_eq!(disabled.plan.right_recommended_ospf_cost, Some(47));
    assert!(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM network_observation_series WHERE id=$1)"
    )
    .bind(series_id)
    .fetch_one(&db.pool)
    .await
    .unwrap());
    // Completion still verifies the pre-upgrade target (49), even though preview
    // policy now floors it. Migration must never rewrite an authorized job.
    for (side, job) in [
        (TunnelEndpointSide::Left, left_job),
        (TunnelEndpointSide::Right, right_job),
    ] {
        db.repo
            .record_tunnel_plan_ospf_job_result(active.id, side, job, Some(49), true)
            .await
            .unwrap()
            .unwrap();
    }
    assert_eq!(
        db.repo
            .get_tunnel_plan_record(active.id)
            .await
            .unwrap()
            .unwrap()
            .ospf_status,
        "verified"
    );
    db.cleanup().await;
}
