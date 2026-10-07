use super::*;
use crate::model::{
    NetworkAdapterDefinitionView, UpdateNetworkAdapterDefinitionRequest,
    UpdateNetworkAdapterMetadataRequest, UpsertNetworkAdapterDefinitionRequest,
};

fn candidate(adapter: &NetworkAdapterDefinitionView) -> UpsertNetworkAdapterDefinitionRequest {
    UpsertNetworkAdapterDefinitionRequest {
        adapter_kind: adapter.adapter_kind.clone(),
        name: adapter.name.clone(),
        description: adapter.description.clone(),
        definition: adapter.definition.clone(),
    }
}

async fn adapter(db: &PgReliabilityTestDb, id: &str) -> NetworkAdapterDefinitionView {
    db.repo
        .list_network_adapter_definitions(None)
        .await
        .unwrap()
        .into_iter()
        .find(|row| row.id.to_string() == id)
        .unwrap()
}

async fn queued(db: &PgReliabilityTestDb) -> Vec<(String, String)> {
    sqlx::query_as(
        "SELECT client_id,reason FROM client_runtime_config_reconcile_work ORDER BY client_id",
    )
    .fetch_all(&db.pool)
    .await
    .unwrap()
}

async fn clear_queued(db: &PgReliabilityTestDb) {
    sqlx::query("DELETE FROM client_runtime_config_reconcile_work")
        .execute(&db.pool)
        .await
        .unwrap();
}

#[test]
fn adapter_update_body_accepts_review_but_metadata_rejects_commands() {
    let body = json!({"adapter_kind":"runtime_tunnel","name":"test","description":null,
        "definition":{},"review_hash":"review","privilege_assertion":null});
    let request: UpdateNetworkAdapterDefinitionRequest = serde_json::from_value(body).unwrap();
    assert_eq!(request.candidate.name, "test");
    assert_eq!(request.review_hash, "review");
    assert!(
        serde_json::from_value::<UpdateNetworkAdapterMetadataRequest>(json!({
            "expected_updated_at":"now","name":"test","description":null,"definition":{}
        }))
        .is_err()
    );
}

#[tokio::test]
async fn postgres_adapter_mutation_metadata_is_nonoperational_and_review_is_fenced() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    for id in ["client-a", "client-b"] {
        insert_client(&db.pool, id, None).await;
    }
    let operator = postgres_network_operator(&db.repo).await;
    let input = crate::tests_network::test_plan_input(RuntimeTunnelManager::CustomAdapter, false);
    crate::tests_network::seed_test_plan_adapter_definitions(&db.repo, &input).await;
    let saved = db
        .repo
        .record_tunnel_plan(&input, &plan_tunnel(&input).unwrap(), true, &operator)
        .await
        .unwrap();
    let old = adapter(
        &db,
        input
            .runtime_control
            .left_adapter_definition_id
            .as_deref()
            .unwrap(),
    )
    .await;
    let mut commands = candidate(&old);
    commands.definition["startup_command"]["argv"] = json!(["/usr/bin/new-start"]);
    let preview = db
        .repo
        .preview_network_adapter_definition(old.id, &commands)
        .await
        .unwrap();
    assert_eq!(preview.change_kind, "commands");
    assert_eq!(preview.target_client_ids, ["client-a"]);
    assert_eq!(preview.affected_resources[0].resource_id, saved.id);
    assert_eq!(preview.affected_resources[0].resource_name, input.name);
    clear_queued(&db).await;
    let before: Value =
        sqlx::query_scalar("SELECT to_jsonb(plan) FROM tunnel_plans plan WHERE id=$1")
            .bind(saved.id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    let rename = UpdateNetworkAdapterMetadataRequest {
        expected_updated_at: old.updated_at.clone(),
        name: "Renamed adapter".into(),
        description: Some("Display only".into()),
    };
    let renamed = db
        .repo
        .update_network_adapter_metadata(old.id, &rename, &operator)
        .await
        .unwrap();
    assert_eq!(renamed.definition, old.definition);
    assert!(queued(&db).await.is_empty());
    let after: Value =
        sqlx::query_scalar("SELECT to_jsonb(plan) FROM tunnel_plans plan WHERE id=$1")
            .bind(saved.id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(after, before);
    assert!(db
        .repo
        .update_network_adapter_metadata(old.id, &rename, &operator)
        .await
        .unwrap_err()
        .to_string()
        .contains("review_stale"));
    assert!(db
        .repo
        .update_network_adapter_definition(old.id, &commands, &operator, &preview.review_hash)
        .await
        .unwrap_err()
        .to_string()
        .contains("review_stale"));
    commands.name = renamed.name.clone();
    commands.description = renamed.description.clone();
    let reviewed = db
        .repo
        .preview_network_adapter_definition(old.id, &commands)
        .await
        .unwrap();
    let mut locked = db.pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM tunnel_plans WHERE id=$1 FOR UPDATE")
        .bind(saved.id)
        .fetch_one(&mut *locked)
        .await
        .unwrap();
    assert!(db
        .repo
        .update_network_adapter_definition(old.id, &commands, &operator, &reviewed.review_hash)
        .await
        .unwrap_err()
        .to_string()
        .contains("review_stale"));
    locked.rollback().await.unwrap();
    let changed = db
        .repo
        .update_network_adapter_definition(old.id, &commands, &operator, &reviewed.review_hash)
        .await
        .unwrap();
    assert_eq!(changed.definition.name, renamed.name);
    assert_eq!(changed.sync.len(), 1);
    assert_eq!(changed.sync[0].status, "queued");
    assert_eq!(
        queued(&db).await,
        [(
            "client-a".into(),
            "network_adapter_definition_updated".into()
        )]
    );
    let plan_state: (bool, i64) =
        sqlx::query_as("SELECT enabled,revision FROM tunnel_plans WHERE id=$1")
            .bind(saved.id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(plan_state, (true, saved.revision));
    assert!(db
        .repo
        .delete_network_adapter_definition(old.id, &operator)
        .await
        .unwrap_err()
        .to_string()
        .contains("in_use"));
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_adapter_mutation_binding_changes_require_review_and_disabled_stays_disabled() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    for id in ["client-a", "client-b"] {
        insert_client(&db.pool, id, None).await;
    }
    let operator = postgres_network_operator(&db.repo).await;
    let input = crate::tests_network::test_plan_input(RuntimeTunnelManager::CustomAdapter, false);
    crate::tests_network::seed_test_plan_adapter_definitions(&db.repo, &input).await;
    let saved = db
        .repo
        .record_tunnel_plan(&input, &plan_tunnel(&input).unwrap(), true, &operator)
        .await
        .unwrap();
    let old = adapter(
        &db,
        input
            .runtime_control
            .left_adapter_definition_id
            .as_deref()
            .unwrap(),
    )
    .await;
    let mut commands = candidate(&old);
    commands.definition["cleanup_command"]["argv"] = json!(["/usr/bin/new-cleanup"]);
    let preview = db
        .repo
        .preview_network_adapter_definition(old.id, &commands)
        .await
        .unwrap();
    let disabled = db
        .repo
        .set_tunnel_plan_enabled(saved.id, saved.revision, false, &operator)
        .await
        .unwrap();
    clear_queued(&db).await;
    assert!(db
        .repo
        .update_network_adapter_definition(old.id, &commands, &operator, &preview.review_hash)
        .await
        .unwrap_err()
        .to_string()
        .contains("review_stale"));
    let fresh = db
        .repo
        .preview_network_adapter_definition(old.id, &commands)
        .await
        .unwrap();
    assert_eq!(fresh.affected_resources.len(), 1);
    assert!(!fresh.affected_resources[0].enabled);
    assert!(fresh.target_client_ids.is_empty());
    let changed = db
        .repo
        .update_network_adapter_definition(old.id, &commands, &operator, &fresh.review_hash)
        .await
        .unwrap();
    assert!(changed.sync.is_empty());
    assert!(queued(&db).await.is_empty());
    let state: (bool, i64) =
        sqlx::query_as("SELECT enabled,revision FROM tunnel_plans WHERE id=$1")
            .bind(saved.id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(state, (false, disabled.revision));
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_adapter_mutation_routing_invalidates_only_matching_job_association() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    for id in ["client-a", "client-b"] {
        insert_client(&db.pool, id, None).await;
    }
    let operator = postgres_network_operator(&db.repo).await;
    let input = crate::tests_network::test_plan_input(RuntimeTunnelManager::AgentBuiltin, true);
    crate::tests_network::seed_test_plan_adapter_definitions(&db.repo, &input).await;
    let saved = db
        .repo
        .record_tunnel_plan(&input, &plan_tunnel(&input).unwrap(), true, &operator)
        .await
        .unwrap();
    let old = adapter(
        &db,
        input
            .ospf
            .as_ref()
            .unwrap()
            .left_adapter_definition_id
            .as_deref()
            .unwrap(),
    )
    .await;
    let left_job = Uuid::new_v4();
    let right_job = Uuid::new_v4();
    sqlx::query("UPDATE tunnel_plans SET ospf_status='pending',left_ospf_status='pending',right_ospf_status='verified',left_current_ospf_cost=17,right_current_ospf_cost=17,left_ospf_job_id=$2,right_ospf_job_id=$3 WHERE id=$1")
        .bind(saved.id).bind(left_job).bind(right_job).execute(&db.pool).await.unwrap();
    let mut commands = candidate(&old);
    commands.definition["update_command"]["argv"] = json!(["/usr/bin/new-cost-update"]);
    let preview = db
        .repo
        .preview_network_adapter_definition(old.id, &commands)
        .await
        .unwrap();
    assert_eq!(preview.target_client_ids, ["client-a"]);
    clear_queued(&db).await;
    let changed = db
        .repo
        .update_network_adapter_definition(old.id, &commands, &operator, &preview.review_hash)
        .await
        .unwrap();
    assert!(changed.sync.is_empty());
    assert!(queued(&db).await.is_empty());
    let row = sqlx::query("SELECT enabled,revision,plan,left_ospf_status,right_ospf_status,left_current_ospf_cost,right_current_ospf_cost,left_ospf_job_id,right_ospf_job_id FROM tunnel_plans WHERE id=$1")
        .bind(saved.id).fetch_one(&db.pool).await.unwrap();
    assert!(row.get::<bool, _>("enabled"));
    assert_eq!(row.get::<i64, _>("revision"), saved.revision + 1);
    assert_eq!(
        row.get::<Value, _>("plan"),
        serde_json::to_value(&saved.plan).unwrap()
    );
    assert_eq!(row.get::<String, _>("left_ospf_status"), "unverified");
    assert_eq!(row.get::<Option<i32>, _>("left_current_ospf_cost"), None);
    assert_eq!(row.get::<Option<Uuid>, _>("left_ospf_job_id"), None);
    assert_eq!(row.get::<String, _>("right_ospf_status"), "verified");
    assert_eq!(
        row.get::<Option<i32>, _>("right_current_ospf_cost"),
        Some(17)
    );
    assert_eq!(
        row.get::<Option<Uuid>, _>("right_ospf_job_id"),
        Some(right_job)
    );
    assert!(db
        .repo
        .record_tunnel_plan_ospf_job_result(
            saved.id,
            TunnelEndpointSide::Left,
            left_job,
            Some(17),
            true
        )
        .await
        .unwrap()
        .is_none());

    // A request can resolve the old executable while costs are already NULL,
    // then reach staging only after a second adapter edit. Cost equality alone
    // cannot fence that request; the existing plan revision must reject it.
    let before_second_edit = db.repo.get_tunnel_plan(saved.id).await.unwrap().unwrap();
    assert_eq!(before_second_edit.left_current_ospf_cost, None);
    let mut next_commands = candidate(&changed.definition);
    next_commands.definition["status_command"]["argv"] = json!(["/usr/bin/new-cost-status"]);
    let next_preview = db
        .repo
        .preview_network_adapter_definition(old.id, &next_commands)
        .await
        .unwrap();
    db.repo
        .update_network_adapter_definition(
            old.id,
            &next_commands,
            &operator,
            &next_preview.review_hash,
        )
        .await
        .unwrap();
    let rejected = db
        .repo
        .stage_tunnel_plan_ospf_jobs(
            saved.id,
            before_second_edit.revision,
            None,
            Some(17),
            None,
            Uuid::new_v4(),
            Uuid::new_v4(),
            &operator,
        )
        .await
        .unwrap_err();
    assert_eq!(rejected.to_string(), "tunnel_plan_ospf_snapshot_stale");
    let latest = db.repo.get_tunnel_plan(saved.id).await.unwrap().unwrap();
    assert_eq!(latest.revision, before_second_edit.revision + 1);
    assert_eq!(latest.left_current_ospf_cost, None);
    assert_eq!(latest.right_current_ospf_cost, Some(17));
    assert_eq!(latest.right_ospf_status, "verified");
    assert_eq!(latest.right_ospf_job_id, Some(right_job));
    assert!(queued(&db).await.is_empty());
    let staged = db
        .repo
        .stage_tunnel_plan_ospf_jobs(
            saved.id,
            latest.revision,
            None,
            Some(17),
            None,
            Uuid::new_v4(),
            Uuid::new_v4(),
            &operator,
        )
        .await
        .unwrap();
    assert_eq!(staged.left_ospf_status, "pending");
    assert_eq!(staged.right_ospf_status, "pending");
    assert_eq!(staged.revision, latest.revision);
    assert!(queued(&db).await.is_empty());
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_adapter_mutation_forwarding_includes_pending_cleanup_without_disabling_rules() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    insert_client(&db.pool, "forward-client", None).await;
    sqlx::query("UPDATE clients SET capabilities=$2 WHERE id=$1")
        .bind("forward-client")
        .bind(SqlJson(json!({"port_forwarding":{"status":"nft_missing","schema_version":3,"supported_modes":["custom_adapter"]}})))
        .execute(&db.pool).await.unwrap();
    let operator = postgres_network_operator(&db.repo).await;
    let command = json!({"argv":["/usr/bin/true","{rule_config_json}"],"max_timeout_secs":30,"max_output_bytes":16384});
    let request = UpsertNetworkAdapterDefinitionRequest {
        adapter_kind: "port_forward".into(),
        name: "external proxy".into(),
        description: None,
        definition: json!({"contract_version":2,"apply_command":command,"remove_command":command,"status_command":command}),
    };
    let old = db
        .repo
        .create_network_adapter_definition(&request, &operator)
        .await
        .unwrap();
    let rule = db.repo.create_port_forward_rule(&serde_json::from_value(json!({"client_id":"forward-client","name":"proxy rule","mode":"custom_adapter","protocol":"tcp","target_ip":"127.0.0.1","adapter_definition_id":old.id,"mappings":pair_port_expressions("18080","8080").unwrap(),"enabled":true,"confirmed":true})).unwrap(), &operator).await.unwrap();
    let original_config = db
        .repo
        .port_forwarding_config_for_client("forward-client")
        .await
        .unwrap();
    let mut commands = candidate(&old);
    commands.definition["apply_command"]["argv"] =
        json!(["/usr/bin/new-proxy", "{rule_config_json}"]);
    let preview = db
        .repo
        .preview_network_adapter_definition(old.id, &commands)
        .await
        .unwrap();
    assert_eq!(preview.target_client_ids, ["forward-client"]);
    assert_eq!(preview.affected_resources[0].resource_id, rule.id);
    clear_queued(&db).await;
    db.repo
        .update_network_adapter_definition(old.id, &commands, &operator, &preview.review_hash)
        .await
        .unwrap();
    assert_eq!(
        queued(&db).await,
        [(
            "forward-client".into(),
            "network_adapter_definition_updated".into()
        )]
    );
    let current = db
        .repo
        .port_forwarding_config_for_client("forward-client")
        .await
        .unwrap();
    assert!(
        db.repo
            .get_port_forward_rule(rule.id)
            .await
            .unwrap()
            .unwrap()
            .enabled
    );
    assert_eq!(current.rules[0].revision, rule.revision);
    assert_ne!(
        current.rules[0].adapter.as_ref().unwrap().definition_hash,
        original_config.rules[0]
            .adapter
            .as_ref()
            .unwrap()
            .definition_hash
    );
    let native: UpdatePortForwardRuleRequest = serde_json::from_value(json!({"expected_revision":rule.revision,"name":"native now","mode":"dnat","protocol":"tcp","target_ip":"192.0.2.90","mappings":rule.mappings,"enabled":true,"confirmed":true})).unwrap();
    db.repo
        .update_port_forward_rule(rule.id, &native, &operator)
        .await
        .unwrap();
    commands.definition["status_command"]["argv"] =
        json!(["/usr/bin/new-status", "{rule_config_json}"]);
    let cleanup_review = db
        .repo
        .preview_network_adapter_definition(old.id, &commands)
        .await
        .unwrap();
    assert_eq!(cleanup_review.target_client_ids, ["forward-client"]);
    assert!(!cleanup_review.affected_resources[0].enabled);
    assert!(cleanup_review.affected_resources[0].cleanup_pending);
    clear_queued(&db).await;
    db.repo
        .update_network_adapter_definition(
            old.id,
            &commands,
            &operator,
            &cleanup_review.review_hash,
        )
        .await
        .unwrap();
    assert_eq!(
        queued(&db).await,
        [(
            "forward-client".into(),
            "network_adapter_definition_updated".into()
        )]
    );
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_adapter_mutation_current_forwarding_definition_restores_bound_rules() {
    use crate::model_port_forwarding::PortForwardRuleListItem;

    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    insert_client(&db.pool, "forward-client", None).await;
    let operator = postgres_network_operator(&db.repo).await;
    let command = |action| {
        json!({
            "argv":["/opt/operator/forwarding",action,"{forwarding_type}","{rule_config_json}"],
            "max_timeout_secs":60,"max_output_bytes":16384
        })
    };
    let request = UpsertNetworkAdapterDefinitionRequest {
        adapter_kind: "port_forward".into(),
        name: "external proxy".into(),
        description: None,
        definition: json!({"contract_version":2,"apply_command":command("apply"),
            "remove_command":command("remove"),"status_command":command("status")}),
    };
    let adapter = db
        .repo
        .create_network_adapter_definition(&request, &operator)
        .await
        .unwrap();
    let custom = db.repo.create_port_forward_rule(&serde_json::from_value(json!({
        "client_id":"forward-client","name":"proxy rule","mode":"custom_adapter",
        "protocol":"tcp","target_ip":"127.0.0.1","adapter_definition_id":adapter.id,
        "mappings":pair_port_expressions("18080","8080").unwrap(),"enabled":true,"confirmed":true
    })).unwrap(), &operator).await.unwrap();
    let redirect = db.repo.create_port_forward_rule(&serde_json::from_value(json!({
        "client_id":"forward-client","name":"local redirect","mode":"custom_adapter",
        "protocol":"tcp","target_ip":"127.0.0.1","adapter_definition_id":adapter.id,
        "mappings":pair_port_expressions("18081","8081").unwrap(),"enabled":true,"confirmed":true
    })).unwrap(), &operator).await.unwrap();
    let redirect = db
        .repo
        .update_port_forward_rule(
            redirect.id,
            &serde_json::from_value(json!({
                "expected_revision":redirect.revision,"name":redirect.name,"mode":"redirect",
                "protocol":"tcp","address_family":"ipv4","mappings":redirect.mappings,
                "enabled":true,"confirmed":true
            }))
            .unwrap(),
            &operator,
        )
        .await
        .unwrap();
    assert!(redirect.adapter_definition_id.is_none());
    assert!(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM port_forward_adapter_owners WHERE rule_id=$1)"
    )
    .bind(redirect.id)
    .fetch_one(&db.pool)
    .await
    .unwrap());
    let before: Vec<Value> =
        sqlx::query_scalar("SELECT to_jsonb(rule) FROM port_forward_rules rule ORDER BY id")
            .fetch_all(&db.pool)
            .await
            .unwrap();

    // Both an ordinary reviewed edit and a direct definition update must restore
    // the existing bindings. Neither needs a rule rewrite or a version fallback.
    for reviewed_edit in [true, false] {
        let mut invalid = request.clone();
        invalid.definition["contract_version"] = json!(1);
        sqlx::query("UPDATE network_adapter_definitions SET definition=$2,updated_at=clock_timestamp() WHERE id=$1")
            .bind(adapter.id).bind(SqlJson(&invalid.definition)).execute(&db.pool).await.unwrap();
        let items = db.repo.list_port_forward_rule_items().await.unwrap();
        assert!(items.iter().any(|item| matches!(item, PortForwardRuleListItem::Corrupt(rule)
            if rule.id == custom.id && rule.configuration_error.contains("port_forward_adapter_definition_invalid"))));
        assert!(items
            .iter()
            .any(|item| matches!(item, PortForwardRuleListItem::Rule(rule)
            if rule.id == redirect.id)));
        let sync_error = db
            .repo
            .port_forwarding_config_for_client("forward-client")
            .await
            .unwrap_err()
            .to_string();
        assert!(sync_error.contains(&format!(
            "port_forward_rule_configuration_corrupt:{}:forward-client:port_forward_adapter_definition_invalid",
            custom.id
        )));
        assert!(db
            .repo
            .preview_network_adapter_definition(adapter.id, &invalid)
            .await
            .unwrap_err()
            .to_string()
            .contains("network_adapter_contract_version_invalid"));
        clear_queued(&db).await;

        if reviewed_edit {
            let preview = db
                .repo
                .preview_network_adapter_definition(adapter.id, &request)
                .await
                .unwrap();
            assert_eq!(preview.target_client_ids, ["forward-client"]);
            assert_eq!(preview.affected_resources.len(), 2);
            assert!(preview
                .affected_resources
                .iter()
                .any(|resource| resource.resource_id == custom.id && resource.enabled));
            assert!(preview
                .affected_resources
                .iter()
                .any(|resource| resource.resource_id == redirect.id
                    && resource.cleanup_pending
                    && !resource.enabled));
            db.repo
                .update_network_adapter_definition(
                    adapter.id,
                    &request,
                    &operator,
                    &preview.review_hash,
                )
                .await
                .unwrap();
        } else {
            sqlx::query("UPDATE network_adapter_definitions SET definition=$2,updated_at=clock_timestamp() WHERE id=$1")
                .bind(adapter.id).bind(SqlJson(&request.definition)).execute(&db.pool).await.unwrap();
        }
        assert_eq!(
            queued(&db).await,
            [(
                "forward-client".into(),
                "network_adapter_definition_updated".into()
            )]
        );
        let items = db.repo.list_port_forward_rule_items().await.unwrap();
        assert_eq!(items.len(), 2);
        assert!(items
            .iter()
            .all(|item| matches!(item, PortForwardRuleListItem::Rule(_))));
        let config = db
            .repo
            .port_forwarding_config_for_client("forward-client")
            .await
            .unwrap();
        assert_eq!(config.rules.len(), 2);
        assert_eq!(config.cleanup_rules.len(), 1);
        assert_eq!(config.cleanup_rules[0].rule_id, redirect.id);
        let commands = config
            .rules
            .iter()
            .find(|rule| rule.id == custom.id)
            .unwrap()
            .adapter
            .as_ref()
            .unwrap();
        assert_eq!(commands.contract_version, 2);
        assert_eq!(
            commands.apply.argv,
            [
                "/opt/operator/forwarding",
                "apply",
                "{forwarding_type}",
                "{rule_config_json}"
            ]
        );
        let after: Vec<Value> =
            sqlx::query_scalar("SELECT to_jsonb(rule) FROM port_forward_rules rule ORDER BY id")
                .fetch_all(&db.pool)
                .await
                .unwrap();
        assert_eq!(after, before);
    }
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_adapter_mutation_route_requires_command_privilege_even_unbound() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let (operator, headers) = postgres_operator_session(&db.repo, "adapter-reviewer").await;
    let request = UpsertNetworkAdapterDefinitionRequest {
        adapter_kind: "routing_cost".into(),
        name: "unbound routing".into(),
        description: None,
        definition: json!({"contract_version":vpsman_common::ROUTING_COST_ADAPTER_CONTRACT_VERSION,"status_command":{"argv":["/usr/bin/status"],"max_timeout_secs":30,"max_output_bytes":16384},"update_command":{"argv":["/usr/bin/update"],"max_timeout_secs":30,"max_output_bytes":16384}}),
    };
    let old = db
        .repo
        .create_network_adapter_definition(&request, &operator)
        .await
        .unwrap();
    let mut commands = candidate(&old);
    commands.definition["update_command"]["argv"] = json!(["/usr/bin/new-update"]);
    let preview = db
        .repo
        .preview_network_adapter_definition(old.id, &commands)
        .await
        .unwrap();
    let mut state = postgres_app_state(&db);
    state.gateway = GatewayDispatchClient::new_with_timeouts(
        Some("http://127.0.0.1:1".into()),
        None,
        Default::default(),
    );
    let result = crate::routes_configuration_presets::update_network_adapter_definition(
        State(state.clone()),
        headers.clone(),
        axum::extract::Path(old.id),
        Json(UpdateNetworkAdapterDefinitionRequest {
            candidate: commands.clone(),
            review_hash: preview.review_hash.clone(),
            privilege_assertion: None,
        }),
    )
    .await;
    let error = result.unwrap_err();
    assert_eq!(error.code, "privilege_assertion_required");
    assert_eq!(
        adapter(&db, &old.id.to_string()).await.definition,
        old.definition
    );
    state.gateway = state.gateway.with_test_privilege_auto_approve();
    let changed = crate::routes_configuration_presets::update_network_adapter_definition(
        State(state),
        headers,
        axum::extract::Path(old.id),
        Json(UpdateNetworkAdapterDefinitionRequest {
            candidate: commands.clone(),
            review_hash: preview.review_hash,
            privilege_assertion: None,
        }),
    )
    .await
    .unwrap();
    assert_eq!(changed.0.definition.definition, commands.definition);
    assert!(changed.0.sync.is_empty());
    db.cleanup().await;
}
