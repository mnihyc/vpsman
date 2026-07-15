use super::*;
use vpsman_common::{
    plan_tunnel, AgentCapabilitySnapshot, AgentHello, AgentPrivilegeMode, CommandOutput,
    OspfCostPolicy, OutputStream, RuntimeTunnelControl, RuntimeTunnelManager,
    RuntimeTunnelTrafficLimit, TunnelKind, TunnelPlanInput,
};

#[tokio::test]
async fn source_templates_assign_defaults_and_shared_custom_templates() {
    let repo = Repository::Memory(MemoryState::default());
    if let Repository::Memory(memory) = &repo {
        for client_id in ["client-a", "client-b"] {
            upsert_memory_agent(
                &memory.agents,
                &AgentHello {
                    client_id: client_id.to_string(),
                    process_incarnation_id: uuid::Uuid::new_v4(),
                    agent_version: "test".to_string(),
                    os_release: "test".to_string(),
                    arch: "x86_64".to_string(),
                    update_heartbeat: None,
                    internal_build_number: 1,
                    capabilities: Default::default(),
                },
            )
            .await;
        }
    }
    let operator = memory_admin();

    let defaults = repo
        .list_source_template_assignments(
            Some("client-a"),
            Some("runtime_traffic_accounting_source"),
        )
        .await
        .unwrap();
    assert_eq!(defaults.len(), 1);
    assert_eq!(defaults[0].template_name, "builtin:interface_counters");

    let vnstat = repo
        .create_source_template(
            &CreateSourceTemplateRequest {
                domain: "runtime_traffic_accounting_source".to_string(),
                name: "shared:vnstat-json".to_string(),
                scope: "shared".to_string(),
                owner_client_id: None,
                description: Some("Provider image with vnstat installed".to_string()),
                definition: serde_json::json!({
                    "source": "vnstat",
                    "traffic_command": {
                        "argv": ["/usr/bin/vnstat", "--json"],
                        "max_timeout_secs": 2,
                        "max_output_bytes": 4096
                    }
                }),
            },
            &operator,
        )
        .await
        .unwrap();

    let preview = repo
        .assign_source_template(
            &AssignSourceTemplateRequest {
                domain: "runtime_traffic_accounting_source".to_string(),
                template_id: vnstat.id,
                selector_expression: "id:client-a || id:client-b".to_string(),
                target_client_ids: vec!["client-a".to_string(), "client-b".to_string()],
                confirmed: false,
                preview_hash: None,
                privilege_assertion: None,
            },
            &operator,
        )
        .await
        .unwrap();
    assert!(preview.confirmation_required);
    assert_eq!(preview.target_count, 2);
    assert!(preview
        .assignments
        .iter()
        .all(|assignment| assignment.template_name == "builtin:interface_counters"));

    let assigned = repo
        .assign_source_template(
            &AssignSourceTemplateRequest {
                domain: "runtime_traffic_accounting_source".to_string(),
                template_id: vnstat.id,
                selector_expression: "id:client-a || id:client-b".to_string(),
                target_client_ids: vec!["client-a".to_string(), "client-b".to_string()],
                confirmed: true,
                preview_hash: None,
                privilege_assertion: None,
            },
            &operator,
        )
        .await
        .unwrap();
    assert!(!assigned.confirmation_required);
    assert_eq!(assigned.assignments.len(), 2);
    assert!(assigned
        .assignments
        .iter()
        .all(|assignment| assignment.template_name == "shared:vnstat-json"));
    assert!(repo
        .list_audit_logs(10)
        .await
        .unwrap()
        .iter()
        .any(|audit| audit.action == "source_template.assigned"));
}

#[tokio::test]
async fn curated_builtin_source_templates_are_selectable_not_default() {
    let repo = Repository::Memory(MemoryState::default());
    if let Repository::Memory(memory) = &repo {
        upsert_memory_agent(
            &memory.agents,
            &AgentHello {
                client_id: "edge-a".to_string(),
                process_incarnation_id: uuid::Uuid::new_v4(),
                agent_version: "test".to_string(),
                os_release: "test".to_string(),
                arch: "x86_64".to_string(),
                update_heartbeat: None,
                internal_build_number: 1,
                capabilities: Default::default(),
            },
        )
        .await;
    }
    let operator = memory_admin();

    let templates = repo.list_source_templates(None).await.unwrap();
    let domains = crate::source_template_builtins::SOURCE_TEMPLATE_DOMAINS;
    assert!(templates.len() > domains.len());
    for domain in domains
        .iter()
        .filter(|domain| !matches!(**domain, "runtime_tunnel_adapter" | "routing_cost_adapter"))
    {
        let defaults = templates
            .iter()
            .filter(|template| template.domain == **domain && template.is_default)
            .collect::<Vec<_>>();
        assert_eq!(
            defaults.len(),
            1,
            "expected one default template for {domain}"
        );
    }

    let default_assignments = repo
        .list_source_template_assignments(Some("edge-a"), None)
        .await
        .unwrap();
    assert_eq!(default_assignments.len(), domains.len() - 2);
    assert!(default_assignments
        .iter()
        .all(|assignment| assignment.template_scope == "built_in"));
    assert!(!default_assignments
        .iter()
        .any(|assignment| assignment.template_name == "builtin:vnstat_json"));

    let curated_names = [
        "builtin:host_mounted_procfs",
        "builtin:vnstat_json",
        "builtin:usr_bin_ping",
        "builtin:usr_bin_w",
        "builtin:busybox_ash_argv",
        "builtin:s3_path_style_reserved",
        "builtin:github_release_sha256",
    ];
    for name in curated_names {
        let template = templates
            .iter()
            .find(|template| template.name == name)
            .unwrap_or_else(|| panic!("missing curated template {name}"));
        assert!(template.built_in);
        assert!(!template.is_default);
    }

    let assignments = [
        (
            "telemetry_metrics_source",
            "builtin:host_mounted_procfs",
            "/host/proc",
        ),
        (
            "process_inventory_source",
            "builtin:host_mounted_procfs",
            "/host/proc",
        ),
        (
            "runtime_traffic_accounting_source",
            "builtin:vnstat_json",
            "/usr/bin/vnstat",
        ),
        (
            "latency_probe_source",
            "builtin:usr_bin_ping",
            "/usr/bin/ping",
        ),
        (
            "user_session_inventory_source",
            "builtin:usr_bin_w",
            "/usr/bin/w",
        ),
        (
            "command_execution_policy",
            "builtin:busybox_ash_argv",
            "/bin/ash",
        ),
    ];
    for (domain, template_name, _) in assignments {
        let template = templates
            .iter()
            .find(|template| template.domain == domain && template.name == template_name)
            .unwrap();
        repo.assign_source_template(
            &AssignSourceTemplateRequest {
                domain: domain.to_string(),
                template_id: template.id,
                selector_expression: "id:edge-a".to_string(),
                target_client_ids: vec!["edge-a".to_string()],
                confirmed: true,
                preview_hash: None,
                privilege_assertion: None,
            },
            &operator,
        )
        .await
        .unwrap();
    }

    let rendered = repo.render_template_runtime_config("edge-a").await.unwrap();
    for (_, _, expected) in assignments {
        assert!(
            rendered.toml.contains(expected),
            "rendered config patch missing {expected}:\n{}",
            rendered.toml
        );
    }

    for template_name in [
        "builtin:s3_path_style_reserved",
        "builtin:github_release_sha256",
    ] {
        let template = templates
            .iter()
            .find(|template| template.name == template_name)
            .unwrap();
        let tested = repo
            .test_source_template(
                template.id,
                &TestSourceTemplateRequest {
                    definition: template.definition.clone(),
                },
            )
            .await
            .unwrap();
        assert!(tested.valid);
        assert!(!tested.renderable);
        assert_eq!(tested.unsupported_domains.len(), 1);
    }
}

#[tokio::test]
async fn adapter_templates_cannot_become_ambient_vps_assignments() {
    let repo = Repository::Memory(MemoryState::default());
    if let Repository::Memory(memory) = &repo {
        upsert_memory_agent(
            &memory.agents,
            &AgentHello {
                client_id: "edge-a".to_string(),
                process_incarnation_id: uuid::Uuid::new_v4(),
                agent_version: "test".to_string(),
                os_release: "test".to_string(),
                arch: "x86_64".to_string(),
                update_heartbeat: None,
                internal_build_number: 1,
                capabilities: Default::default(),
            },
        )
        .await;
    }
    let operator = memory_admin();
    for domain in ["runtime_tunnel_adapter", "routing_cost_adapter"] {
        let template = repo
            .create_source_template(
                &CreateSourceTemplateRequest {
                    domain: domain.to_string(),
                    name: format!("shared:{domain}"),
                    scope: "shared".to_string(),
                    owner_client_id: None,
                    description: None,
                    definition: serde_json::json!({}),
                },
                &operator,
            )
            .await
            .unwrap();
        let error = repo
            .assign_source_template(
                &AssignSourceTemplateRequest {
                    domain: domain.to_string(),
                    template_id: template.id,
                    selector_expression: "id:edge-a".to_string(),
                    target_client_ids: vec!["edge-a".to_string()],
                    confirmed: true,
                    preview_hash: None,
                    privilege_assertion: None,
                },
                &operator,
            )
            .await
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("source_template_adapter_requires_tunnel_plan_binding"));
    }
}

#[tokio::test]
async fn source_template_lifecycle_updates_the_shared_model() {
    let repo = Repository::Memory(MemoryState::default());
    if let Repository::Memory(memory) = &repo {
        for client_id in ["client-a", "client-b"] {
            upsert_memory_agent(
                &memory.agents,
                &AgentHello {
                    client_id: client_id.to_string(),
                    process_incarnation_id: uuid::Uuid::new_v4(),
                    agent_version: "test".to_string(),
                    os_release: "test".to_string(),
                    arch: "x86_64".to_string(),
                    update_heartbeat: None,
                    internal_build_number: 1,
                    capabilities: Default::default(),
                },
            )
            .await;
        }
    }
    let operator = memory_admin();
    let template = repo
        .create_source_template(
            &CreateSourceTemplateRequest {
                domain: "runtime_traffic_accounting_source".to_string(),
                name: "shared:traffic-source".to_string(),
                scope: "shared".to_string(),
                owner_client_id: None,
                description: Some("default traffic source".to_string()),
                definition: serde_json::json!({"source": "interface_counters"}),
            },
            &operator,
        )
        .await
        .unwrap();
    repo.assign_source_template(
        &AssignSourceTemplateRequest {
            domain: "runtime_traffic_accounting_source".to_string(),
            template_id: template.id,
            selector_expression: "id:client-a || id:client-b".to_string(),
            target_client_ids: vec!["client-a".to_string(), "client-b".to_string()],
            confirmed: true,
            preview_hash: None,
            privilege_assertion: None,
        },
        &operator,
    )
    .await
    .unwrap();

    let candidate = serde_json::json!({
        "source": "vnstat",
        "vnstat_argv": ["/usr/bin/vnstat", "--json"]
    });
    let diff = repo
        .diff_source_template(
            template.id,
            &SourceTemplateDiffRequest {
                description: Some("provider image uses vnstat".to_string()),
                definition: candidate.clone(),
                keep_description: false,
            },
        )
        .await
        .unwrap();
    assert_eq!(diff.affected_client_count, 2);
    assert_eq!(diff.changed_keys, vec!["source", "vnstat_argv"]);

    let test = repo
        .test_source_template(
            template.id,
            &TestSourceTemplateRequest {
                definition: candidate.clone(),
            },
        )
        .await
        .unwrap();
    assert!(test.valid);
    assert!(test.renderable);
    assert!(test.toml.contains("runtime_vnstat_argv"));

    let preview = repo
        .update_source_template(
            template.id,
            &UpdateSourceTemplateRequest {
                description: Some("provider image uses vnstat".to_string()),
                definition: candidate.clone(),
                confirmed: false,
                keep_description: false,
                preview_hash: None,
                privilege_assertion: None,
            },
            &operator,
        )
        .await
        .unwrap();
    assert!(preview.confirmation_required);
    assert_eq!(preview.affected_client_count, 2);
    assert_eq!(
        preview.template.definition,
        serde_json::json!({"source": "interface_counters"})
    );

    let updated = repo
        .update_source_template(
            template.id,
            &UpdateSourceTemplateRequest {
                description: Some("provider image uses vnstat".to_string()),
                definition: candidate.clone(),
                confirmed: true,
                keep_description: false,
                preview_hash: None,
                privilege_assertion: None,
            },
            &operator,
        )
        .await
        .unwrap();
    assert!(!updated.confirmation_required);
    assert_eq!(updated.template.definition, candidate);
    assert_eq!(updated.template.assigned_client_count, 2);
    assert!(repo
        .list_audit_logs(10)
        .await
        .unwrap()
        .iter()
        .any(|audit| audit.action == "source_template.updated"));
}

#[tokio::test]
async fn source_template_update_route_binds_confirmed_apply_to_preview() {
    let repo = Repository::Memory(MemoryState::default());
    if let Repository::Memory(memory) = &repo {
        for client_id in ["client-a", "client-b"] {
            upsert_memory_agent(
                &memory.agents,
                &AgentHello {
                    client_id: client_id.to_string(),
                    process_incarnation_id: uuid::Uuid::new_v4(),
                    agent_version: "test".to_string(),
                    os_release: "test".to_string(),
                    arch: "x86_64".to_string(),
                    update_heartbeat: None,
                    internal_build_number: 1,
                    capabilities: Default::default(),
                },
            )
            .await;
        }
    }
    let mut state = source_template_test_state(repo.clone(), None);
    state.gateway = GatewayDispatchClient::new(Some("http://127.0.0.1:1".to_string()), None)
        .with_test_privilege_auto_approve();
    let headers = crate::test_auth_headers(&state).await;
    let operator = memory_admin();
    let template = repo
        .create_source_template(
            &CreateSourceTemplateRequest {
                domain: "runtime_traffic_accounting_source".to_string(),
                name: "shared:traffic-route-preview".to_string(),
                scope: "shared".to_string(),
                owner_client_id: None,
                description: Some("default traffic source".to_string()),
                definition: serde_json::json!({"source": "interface_counters"}),
            },
            &operator,
        )
        .await
        .unwrap();
    repo.assign_source_template(
        &AssignSourceTemplateRequest {
            domain: "runtime_traffic_accounting_source".to_string(),
            template_id: template.id,
            selector_expression: "id:client-a || id:client-b".to_string(),
            target_client_ids: vec!["client-a".to_string(), "client-b".to_string()],
            confirmed: true,
            preview_hash: None,
            privilege_assertion: None,
        },
        &operator,
    )
    .await
    .unwrap();

    let candidate = serde_json::json!({
        "source": "vnstat",
        "vnstat_argv": ["/usr/bin/vnstat", "--json"]
    });
    let axum::Json(preview) = routes_inventory::update_source_template(
        axum::extract::State(state.clone()),
        headers.clone(),
        axum::extract::Path(template.id),
        axum::Json(UpdateSourceTemplateRequest {
            description: Some("provider image uses vnstat".to_string()),
            definition: candidate.clone(),
            confirmed: false,
            keep_description: false,
            preview_hash: None,
            privilege_assertion: None,
        }),
    )
    .await
    .unwrap();
    assert!(preview.confirmation_required);
    assert_eq!(
        preview.affected_client_ids,
        vec!["client-a".to_string(), "client-b".to_string()]
    );
    let preview_hash = preview.preview_hash.clone().unwrap();

    let error = routes_inventory::update_source_template(
        axum::extract::State(state.clone()),
        headers.clone(),
        axum::extract::Path(template.id),
        axum::Json(UpdateSourceTemplateRequest {
            description: Some("provider image uses vnstat".to_string()),
            definition: candidate.clone(),
            confirmed: true,
            keep_description: false,
            preview_hash: None,
            privilege_assertion: None,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(error.status, axum::http::StatusCode::CONFLICT);
    assert_eq!(error.code, "source_template_update_preview_hash_required");
    let unchanged = repo
        .list_source_templates(Some("runtime_traffic_accounting_source"))
        .await
        .unwrap()
        .into_iter()
        .find(|row| row.id == template.id)
        .unwrap();
    assert_eq!(
        unchanged.definition,
        serde_json::json!({"source": "interface_counters"})
    );

    let axum::Json(updated) = routes_inventory::update_source_template(
        axum::extract::State(state),
        headers,
        axum::extract::Path(template.id),
        axum::Json(UpdateSourceTemplateRequest {
            description: Some("provider image uses vnstat".to_string()),
            definition: candidate.clone(),
            confirmed: true,
            keep_description: false,
            preview_hash: Some(preview_hash.clone()),
            privilege_assertion: None,
        }),
    )
    .await
    .unwrap();
    assert!(!updated.confirmation_required);
    assert_eq!(updated.preview_hash.as_deref(), Some(preview_hash.as_str()));
    assert_eq!(updated.template.definition, candidate);
}

#[tokio::test]
async fn source_template_assignment_route_binds_confirmed_apply_to_preview() {
    let repo = Repository::Memory(MemoryState::default());
    if let Repository::Memory(memory) = &repo {
        for client_id in ["client-a", "client-b"] {
            upsert_memory_agent(
                &memory.agents,
                &AgentHello {
                    client_id: client_id.to_string(),
                    process_incarnation_id: uuid::Uuid::new_v4(),
                    agent_version: "test".to_string(),
                    os_release: "test".to_string(),
                    arch: "x86_64".to_string(),
                    update_heartbeat: None,
                    internal_build_number: 1,
                    capabilities: Default::default(),
                },
            )
            .await;
        }
    }
    let mut state = source_template_test_state(repo.clone(), None);
    state.gateway = GatewayDispatchClient::new(Some("http://127.0.0.1:1".to_string()), None)
        .with_test_privilege_auto_approve();
    let headers = crate::test_auth_headers(&state).await;
    let template = repo
        .create_source_template(
            &CreateSourceTemplateRequest {
                domain: "runtime_traffic_accounting_source".to_string(),
                name: "shared:traffic-assignment-preview".to_string(),
                scope: "shared".to_string(),
                owner_client_id: None,
                description: Some("vnstat traffic source".to_string()),
                definition: serde_json::json!({"source": "vnstat"}),
            },
            &memory_admin(),
        )
        .await
        .unwrap();

    let axum::Json(preview) = routes_inventory::assign_source_template(
        axum::extract::State(state.clone()),
        headers.clone(),
        axum::Json(AssignSourceTemplateRequest {
            domain: "runtime_traffic_accounting_source".to_string(),
            template_id: template.id,
            selector_expression: "id:client-a || id:client-b".to_string(),
            target_client_ids: vec!["client-b".to_string(), "client-a".to_string()],
            confirmed: false,
            preview_hash: None,
            privilege_assertion: None,
        }),
    )
    .await
    .unwrap();
    assert!(preview.confirmation_required);
    assert_eq!(preview.target_count, 2);
    let preview_hash = preview.preview_hash.clone().unwrap();

    let error = routes_inventory::assign_source_template(
        axum::extract::State(state.clone()),
        headers.clone(),
        axum::Json(AssignSourceTemplateRequest {
            domain: "runtime_traffic_accounting_source".to_string(),
            template_id: template.id,
            selector_expression: "id:client-a || id:client-b".to_string(),
            target_client_ids: vec!["client-a".to_string(), "client-b".to_string()],
            confirmed: true,
            preview_hash: None,
            privilege_assertion: None,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(error.status, axum::http::StatusCode::CONFLICT);
    assert_eq!(
        error.code,
        "source_template_assignment_preview_hash_required"
    );

    let axum::Json(assigned) = routes_inventory::assign_source_template(
        axum::extract::State(state),
        headers,
        axum::Json(AssignSourceTemplateRequest {
            domain: "runtime_traffic_accounting_source".to_string(),
            template_id: template.id,
            selector_expression: "id:client-a || id:client-b".to_string(),
            target_client_ids: vec!["client-a".to_string(), "client-b".to_string()],
            confirmed: true,
            preview_hash: Some(preview_hash.clone()),
            privilege_assertion: None,
        }),
    )
    .await
    .unwrap();
    assert!(!assigned.confirmation_required);
    assert_eq!(
        assigned.preview_hash.as_deref(),
        Some(preview_hash.as_str())
    );
    assert!(assigned
        .assignments
        .iter()
        .all(|assignment| assignment.template_id == template.id));
}

#[tokio::test]
async fn source_template_clone_keeps_assignment_separate() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = memory_admin();
    let builtins = repo
        .list_source_templates(Some("command_execution_policy"))
        .await
        .unwrap();
    let source = builtins
        .iter()
        .find(|template| template.name == "builtin:linux_shell_argv")
        .unwrap();
    let clone = repo
        .clone_source_template(
            source.id,
            &CloneSourceTemplateRequest {
                name: "shared:site-shell-policy".to_string(),
                scope: "shared".to_string(),
                owner_client_id: None,
                description: Some("site command execution policy".to_string()),
            },
            &operator,
        )
        .await
        .unwrap();
    assert_eq!(clone.domain, source.domain);
    assert_eq!(clone.definition, source.definition);
    assert!(!clone.built_in);
    assert_eq!(clone.assigned_client_count, 0);
    assert!(repo
        .list_audit_logs(10)
        .await
        .unwrap()
        .iter()
        .any(|audit| audit.action == "source_template.cloned"));
}

#[tokio::test]
async fn vps_local_source_template_only_assigns_to_owner() {
    let repo = Repository::Memory(MemoryState::default());
    if let Repository::Memory(memory) = &repo {
        for client_id in ["client-a", "client-b"] {
            upsert_memory_agent(
                &memory.agents,
                &AgentHello {
                    client_id: client_id.to_string(),
                    process_incarnation_id: uuid::Uuid::new_v4(),
                    agent_version: "test".to_string(),
                    os_release: "test".to_string(),
                    arch: "x86_64".to_string(),
                    update_heartbeat: None,
                    internal_build_number: 1,
                    capabilities: Default::default(),
                },
            )
            .await;
        }
    }
    let operator = memory_admin();
    let template = repo
        .create_source_template(
            &CreateSourceTemplateRequest {
                domain: "process_inventory_source".to_string(),
                name: "local:mounted-host-proc".to_string(),
                scope: "vps_local".to_string(),
                owner_client_id: Some("client-a".to_string()),
                description: None,
                definition: serde_json::json!({
                    "source": "linux_procfs",
                    "proc_root": "/host/proc"
                }),
            },
            &operator,
        )
        .await
        .unwrap();

    let error = repo
        .assign_source_template(
            &AssignSourceTemplateRequest {
                domain: "process_inventory_source".to_string(),
                template_id: template.id,
                selector_expression: "id:client-b".to_string(),
                target_client_ids: vec!["client-b".to_string()],
                confirmed: true,
                preview_hash: None,
                privilege_assertion: None,
            },
            &operator,
        )
        .await
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("vps_local_template_owner_mismatch"));

    let assigned = repo
        .assign_source_template(
            &AssignSourceTemplateRequest {
                domain: "process_inventory_source".to_string(),
                template_id: template.id,
                selector_expression: "id:client-a".to_string(),
                target_client_ids: vec!["client-a".to_string()],
                confirmed: true,
                preview_hash: None,
                privilege_assertion: None,
            },
            &operator,
        )
        .await
        .unwrap();
    assert_eq!(assigned.assignments[0].client_id, "client-a");
    assert_eq!(
        assigned.assignments[0].template_name,
        "local:mounted-host-proc"
    );
}

#[tokio::test]
async fn template_runtime_config_renders_selected_templates() {
    let repo = Repository::Memory(MemoryState::default());
    if let Repository::Memory(memory) = &repo {
        upsert_memory_agent(
            &memory.agents,
            &AgentHello {
                client_id: "edge-a".to_string(),
                process_incarnation_id: uuid::Uuid::new_v4(),
                agent_version: "test".to_string(),
                os_release: "test".to_string(),
                arch: "x86_64".to_string(),
                update_heartbeat: None,
                internal_build_number: 1,
                capabilities: Default::default(),
            },
        )
        .await;
    }
    let operator = memory_admin();

    let telemetry = repo
        .create_source_template(
            &CreateSourceTemplateRequest {
                domain: "telemetry_metrics_source".to_string(),
                name: "shared:custom-metrics".to_string(),
                scope: "shared".to_string(),
                owner_client_id: None,
                description: None,
                definition: serde_json::json!({
                    "source": "linux_procfs_and_custom_command",
                    "proc_root": "/proc",
                    "custom_metrics_command": {
                        "argv": ["/opt/vpsman/metrics"],
                        "max_timeout_secs": 3,
                        "max_output_bytes": 4096
                    }
                }),
            },
            &operator,
        )
        .await
        .unwrap();
    let process = repo
        .create_source_template(
            &CreateSourceTemplateRequest {
                domain: "process_inventory_source".to_string(),
                name: "shared:processctl".to_string(),
                scope: "shared".to_string(),
                owner_client_id: None,
                description: None,
                definition: serde_json::json!({
                    "source": "custom_command",
                    "process_inventory_command": {
                        "argv": ["/opt/vpsman/process-inventory"],
                        "max_timeout_secs": 5,
                        "max_output_bytes": 8192
                    }
                }),
            },
            &operator,
        )
        .await
        .unwrap();
    let vnstat = repo
        .create_source_template(
            &CreateSourceTemplateRequest {
                domain: "runtime_traffic_accounting_source".to_string(),
                name: "shared:vnstat".to_string(),
                scope: "shared".to_string(),
                owner_client_id: None,
                description: None,
                definition: serde_json::json!({
                    "source": "vnstat",
                    "vnstat_argv": ["/usr/bin/vnstat"]
                }),
            },
            &operator,
        )
        .await
        .unwrap();
    let execution = repo
        .create_source_template(
            &CreateSourceTemplateRequest {
                domain: "command_execution_policy".to_string(),
                name: "shared:clean-batch".to_string(),
                scope: "shared".to_string(),
                owner_client_id: None,
                description: None,
                definition: serde_json::json!({
                    "shell_script_argv": ["/bin/sh", "-lc"],
                    "working_directory": "/tmp",
                    "environment_policy": "clean",
                    "environment_keep": ["PATH", "HOME"],
                    "environment_set": {"VPSMAN_EXECUTION_MODE": "batch"},
                    "pty_policy": "disabled",
                    "process_cleanup": "direct_child"
                }),
            },
            &operator,
        )
        .await
        .unwrap();

    for (domain, template_id) in [
        ("telemetry_metrics_source", telemetry.id),
        ("process_inventory_source", process.id),
        ("runtime_traffic_accounting_source", vnstat.id),
        ("command_execution_policy", execution.id),
    ] {
        repo.assign_source_template(
            &AssignSourceTemplateRequest {
                domain: domain.to_string(),
                template_id,
                selector_expression: "id:edge-a".to_string(),
                target_client_ids: vec!["edge-a".to_string()],
                confirmed: true,
                preview_hash: None,
                privilege_assertion: None,
            },
            &operator,
        )
        .await
        .unwrap();
    }

    let rendered = repo.render_template_runtime_config("edge-a").await.unwrap();
    assert_eq!(rendered.client_id, "edge-a");
    assert!(rendered.toml.contains("[telemetry]"));
    assert!(rendered
        .toml
        .contains("source = \"linux_procfs_and_custom_command\""));
    assert!(rendered
        .toml
        .contains("[execution.process_inventory_command]"));
    assert!(rendered
        .toml
        .contains("process_inventory_source = \"custom_command\""));
    assert!(rendered
        .toml
        .contains("runtime_vnstat_argv = [\"/usr/bin/vnstat\"]"));
    assert!(rendered.toml.contains("working_directory = \"/tmp\""));
    assert!(rendered.toml.contains("environment_policy = \"clean\""));
    assert!(rendered.toml.contains("pty_policy = \"disabled\""));
    assert!(rendered.toml.contains("process_cleanup = \"direct_child\""));
    assert_eq!(
        rendered.sections["execution"]["environment_keep"],
        serde_json::json!(["PATH", "HOME"])
    );
    assert!(rendered
        .unsupported_domains
        .iter()
        .any(|domain| domain.starts_with("backup_object_store:")));
    let rendered_domains = rendered
        .assignments
        .iter()
        .map(|assignment| assignment.domain.as_str())
        .collect::<std::collections::HashSet<_>>();
    for required_domain in [
        "telemetry_metrics_source",
        "process_inventory_source",
        "runtime_traffic_accounting_source",
        "command_execution_policy",
    ] {
        assert!(rendered_domains.contains(required_domain));
    }
}

#[tokio::test]
async fn template_runtime_config_rejects_unsafe_migrated_template_commands() {
    let repo = Repository::Memory(MemoryState::default());
    if let Repository::Memory(memory) = &repo {
        upsert_memory_agent(
            &memory.agents,
            &AgentHello {
                client_id: "edge-a".to_string(),
                process_incarnation_id: uuid::Uuid::new_v4(),
                agent_version: "test".to_string(),
                os_release: "test".to_string(),
                arch: "x86_64".to_string(),
                update_heartbeat: None,
                internal_build_number: 1,
                capabilities: Default::default(),
            },
        )
        .await;
        memory
            .source_templates
            .write()
            .await
            .push(SourceTemplateView {
                id: Uuid::new_v4(),
                domain: "command_execution_policy".to_string(),
                name: "shared:bad-shell".to_string(),
                scope: "shared".to_string(),
                built_in: false,
                is_default: false,
                owner_client_id: None,
                description: None,
                definition: serde_json::json!({
                    "shell_script_argv": ["sh", "-lc"]
                }),
                assigned_client_count: 1,
                created_at: "0".to_string(),
                updated_at: "0".to_string(),
            });
        let template_id = memory.source_templates.read().await.last().unwrap().id;
        memory
            .source_template_assignments
            .write()
            .await
            .push(SourceTemplateAssignmentView {
                client_id: "edge-a".to_string(),
                domain: "command_execution_policy".to_string(),
                template_id,
                template_name: "shared:bad-shell".to_string(),
                template_scope: "shared".to_string(),
                assigned_at: "0".to_string(),
            });
    }

    let error = repo
        .render_template_runtime_config("edge-a")
        .await
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("shell_script_argv_executable_must_be_absolute"));
}

#[tokio::test]
async fn source_status_links_selected_templates_to_live_source_evidence() {
    let repo = Repository::Memory(MemoryState::default());
    if let Repository::Memory(memory) = &repo {
        upsert_memory_agent(
            &memory.agents,
            &AgentHello {
                client_id: "edge-a".to_string(),
                process_incarnation_id: uuid::Uuid::new_v4(),
                agent_version: "test".to_string(),
                os_release: "test".to_string(),
                arch: "x86_64".to_string(),
                update_heartbeat: None,
                internal_build_number: 1,
                capabilities: AgentCapabilitySnapshot {
                    privilege_mode: AgentPrivilegeMode::Root,
                    effective_uid: Some(0),
                    max_job_timeout_secs: 3600,
                    can_attempt_privileged_ops: true,
                    can_manage_runtime_tunnels: true,
                    can_apply_process_limits: true,
                    port_forwarding: Default::default(),
                    unprivileged_hint: None,
                },
            },
        )
        .await;
        upsert_memory_agent(
            &memory.agents,
            &AgentHello {
                client_id: "edge-b".to_string(),
                process_incarnation_id: uuid::Uuid::new_v4(),
                agent_version: "test".to_string(),
                os_release: "test".to_string(),
                arch: "x86_64".to_string(),
                update_heartbeat: None,
                internal_build_number: 1,
                capabilities: AgentCapabilitySnapshot {
                    privilege_mode: AgentPrivilegeMode::Unprivileged,
                    effective_uid: Some(1000),
                    max_job_timeout_secs: 3600,
                    can_attempt_privileged_ops: true,
                    can_manage_runtime_tunnels: false,
                    can_apply_process_limits: false,
                    port_forwarding: Default::default(),
                    unprivileged_hint: Some("running without root in test".to_string()),
                },
            },
        )
        .await;
        memory
            .telemetry_tunnels
            .write()
            .await
            .push(TelemetryTunnelView {
                client_id: "edge-a".to_string(),
                observed_at: "100".to_string(),
                interface: "gre42".to_string(),
                kind: "gre".to_string(),
                ownership_mode: "managed".to_string(),
                mutation_policy: "managed".to_string(),
                plan_id: Some(Uuid::nil()),
                plan_name: Some("edge-a-gre42".to_string()),
                plan_runtime_manager: Some("agent_iproute2_managed".to_string()),
                endpoint_side: Some("left".to_string()),
                peer_client_id: Some("edge-b".to_string()),
                source: "telemetry".to_string(),
                operstate: Some("up".to_string()),
                mtu: Some(1476),
                link_type: Some(778),
                address: Some("10.0.0.1".to_string()),
                rx_bytes: 100,
                tx_bytes: 200,
                traffic_source: Some("vnstat".to_string()),
                traffic_status: Some("ok".to_string()),
                traffic_reason: None,
                traffic_checked_unix: Some(100),
                adapter_health: Some(TelemetryTunnelAdapterHealthView {
                    status: "ok".to_string(),
                    checked_unix: 100,
                    configured: true,
                    success: true,
                    exit_code: Some(0),
                    reason: None,
                    duration_ms: 4,
                    command_sha256_hex: Some("00".repeat(32)),
                    timed_out: false,
                    output_truncated: false,
                    stdout_sha256_hex: None,
                    stderr_sha256_hex: None,
                }),
                latency_monitoring_enabled: None,
                latency_status: None,
                latency_reason: None,
                latency_primary_family: None,
                latency_target: None,
                latency_checked_unix: None,
                latency_avg_ms: None,
                packet_loss_ratio: None,
                latency_healthy_windows: None,
                latency_missed_windows: None,
            });
    }
    let operator = memory_admin();
    let vnstat = repo
        .create_source_template(
            &CreateSourceTemplateRequest {
                domain: "runtime_traffic_accounting_source".to_string(),
                name: "shared:vnstat".to_string(),
                scope: "shared".to_string(),
                owner_client_id: None,
                description: None,
                definition: serde_json::json!({
                    "source": "vnstat",
                    "vnstat_argv": ["/usr/bin/vnstat"]
                }),
            },
            &operator,
        )
        .await
        .unwrap();
    repo.assign_source_template(
        &AssignSourceTemplateRequest {
            domain: "runtime_traffic_accounting_source".to_string(),
            template_id: vnstat.id,
            selector_expression: "id:edge-a".to_string(),
            target_client_ids: vec!["edge-a".to_string()],
            confirmed: true,
            preview_hash: None,
            privilege_assertion: None,
        },
        &operator,
    )
    .await
    .unwrap();

    let runtime_adapter = repo
        .create_source_template(
            &CreateSourceTemplateRequest {
                domain: "runtime_tunnel_adapter".to_string(),
                name: "shared:runtime-adapter".to_string(),
                scope: "shared".to_string(),
                owner_client_id: None,
                description: None,
                definition: serde_json::json!({
                    "manager": "external_managed_adapter",
                    "contract_version": 1,
                    "startup_command": {
                        "argv": ["/usr/local/libexec/tunnel-adapter", "start"],
                        "max_timeout_secs": 10,
                        "max_output_bytes": 4096
                    },
                    "cleanup_command": {
                        "argv": ["/usr/local/libexec/tunnel-adapter", "cleanup"],
                        "max_timeout_secs": 10,
                        "max_output_bytes": 4096
                    },
                    "status_command": {
                        "argv": ["/usr/local/libexec/tunnel-adapter", "status"],
                        "max_timeout_secs": 10,
                        "max_output_bytes": 4096
                    }
                }),
            },
            &operator,
        )
        .await
        .unwrap();
    let routing_adapter = repo
        .create_source_template(
            &CreateSourceTemplateRequest {
                domain: "routing_cost_adapter".to_string(),
                name: "shared:routing-adapter".to_string(),
                scope: "shared".to_string(),
                owner_client_id: None,
                description: None,
                definition: serde_json::json!({
                    "contract_version": 1,
                    "status_command": {
                        "argv": ["/usr/local/libexec/routing-adapter", "status"],
                        "max_timeout_secs": 10,
                        "max_output_bytes": 4096
                    },
                    "update_command": {
                        "argv": ["/usr/local/libexec/routing-adapter", "apply"],
                        "max_timeout_secs": 10,
                        "max_output_bytes": 4096
                    }
                }),
            },
            &operator,
        )
        .await
        .unwrap();
    let tunnel_input = TunnelPlanInput {
        name: "edge-a-gre42".to_string(),
        interface_name: "gre42".to_string(),
        kind: TunnelKind::Gre,
        runtime_control: RuntimeTunnelControl {
            manager: RuntimeTunnelManager::ExternalManagedAdapter,
            left_adapter_template_id: Some(runtime_adapter.id.to_string()),
            right_adapter_template_id: Some(runtime_adapter.id.to_string()),
            ..RuntimeTunnelControl::default()
        },
        runtime_topology: Default::default(),
        left_client_id: "edge-a".to_string(),
        right_client_id: "edge-b".to_string(),
        left_remote_underlay: "198.51.100.10".to_string(),
        right_remote_underlay: "203.0.113.20".to_string(),
        left_local_underlay: None,
        right_local_underlay: None,
        address_pool_cidr: "10.42.0.0/30".to_string(),
        reserved_addresses: Vec::new(),
        ipv4_tunnel: Some(vpsman_common::TunnelAddressPair {
            left: "10.42.0.0".to_string(),
            right: "10.42.0.1".to_string(),
            prefix_len: 31,
        }),
        ipv6_address_pool_cidr: None,
        ipv6_tunnel: None,
        latency_primary_family: Default::default(),
        bandwidth_mbps: 100,
        ospf: None,
    };
    let tunnel_plan = plan_tunnel(&tunnel_input).unwrap();
    let saved_tunnel = repo
        .record_tunnel_plan(&tunnel_input, &tunnel_plan, true, &operator)
        .await
        .unwrap();
    if let Repository::Memory(memory) = &repo {
        memory.telemetry_tunnels.write().await[0].plan_id = Some(saved_tunnel.id);
    }

    let templates = repo.list_source_templates(None).await.unwrap();
    assert_eq!(
        templates
            .iter()
            .find(|template| template.id == runtime_adapter.id)
            .unwrap()
            .assigned_client_count,
        2
    );
    assert_eq!(
        templates
            .iter()
            .find(|template| template.id == routing_adapter.id)
            .unwrap()
            .assigned_client_count,
        0
    );

    let all = repo.list_source_status(Some("edge-a"), None).await.unwrap();
    assert_eq!(
        all.len(),
        crate::source_template_builtins::SOURCE_TEMPLATE_DOMAINS.len() - 2
    );
    assert!(all
        .iter()
        .any(|row| row.domain == "telemetry_metrics_source"
            && row.template_name == "builtin:linux_procfs"
            && row.status == "selected"));
    let process = status_row(&all, "process_inventory_source");
    assert_eq!(process.status, "ready_on_demand");
    assert_eq!(process.evidence["workflow"], "process_inventory");
    assert_eq!(
        process.evidence["supervisor_workflow"],
        "process_supervisor"
    );
    assert_eq!(process.evidence["privilege_gated"], true);
    assert_eq!(process.evidence["privilege_mode"], "root");
    assert_eq!(process.evidence["can_apply_process_limits"], true);
    assert_eq!(process.evidence["process_limits_status"], "available");
    let sessions = status_row(&all, "user_session_inventory_source");
    assert_eq!(sessions.status, "ready_on_demand");
    assert_eq!(sessions.evidence["workflow"], "user_session_inventory");
    let probe = status_row(&all, "latency_probe_source");
    assert_eq!(probe.status, "ready_on_demand");
    assert_eq!(probe.evidence["workflow"], "network_probe");
    let speed = status_row(&all, "speed_test_provider");
    assert_eq!(speed.status, "ready_on_demand");
    assert_eq!(speed.evidence["requires_two_endpoints"], true);
    let execution = status_row(&all, "command_execution_policy");
    assert_eq!(execution.status, "ready_on_demand");
    assert_eq!(execution.evidence["workflow"], "command_execution");
    assert_eq!(execution.evidence["environment_policy"], "inherit");
    assert_eq!(execution.evidence["pty_policy"], "native_pty");
    assert_eq!(execution.evidence["process_cleanup"], "process_group");
    let supervisor = status_row(&all, "process_supervisor_policy");
    assert_eq!(supervisor.status, "ready_on_demand");
    assert_eq!(supervisor.evidence["workflow"], "process_supervisor");
    assert_eq!(supervisor.evidence["process_limits_status"], "available");
    let restore_mapping = status_row(&all, "restore_path_mapping");
    assert_eq!(restore_mapping.status, "ready_on_demand");
    assert_eq!(restore_mapping.evidence["mapping_mode"], "explicit_paths");
    let update_restart = status_row(&all, "update_restart_policy");
    assert_eq!(update_restart.status, "ready_on_demand");
    assert_eq!(
        update_restart.evidence["restart_method"],
        "agent_configured"
    );
    let heartbeat = status_row(&all, "update_rollback_heartbeat_source");
    assert_eq!(heartbeat.status, "ready_on_demand");
    assert_eq!(heartbeat.evidence["health_gate"], "heartbeat_verified");

    let traffic = repo
        .list_source_status(Some("edge-a"), Some("runtime_traffic_accounting_source"))
        .await
        .unwrap();
    assert_eq!(traffic.len(), 1);
    assert_eq!(traffic[0].template_name, "shared:vnstat");
    assert_eq!(traffic[0].source_kind, "vnstat");
    assert_eq!(traffic[0].status, "ok");
    assert_eq!(traffic[0].evidence["sample_count"], 1);

    let tunnels = repo
        .list_source_status(Some("edge-a"), Some("runtime_tunnel_adapter"))
        .await
        .unwrap();
    assert!(tunnels.is_empty());

    let unprivileged_process = repo
        .list_source_status(Some("edge-b"), Some("process_inventory_source"))
        .await
        .unwrap();
    assert_eq!(unprivileged_process.len(), 1);
    assert_eq!(
        unprivileged_process[0].evidence["process_limits_status"],
        "degraded_unprivileged"
    );
    assert_eq!(
        unprivileged_process[0].evidence["privilege_mode"],
        "unprivileged"
    );
    assert_eq!(
        unprivileged_process[0].evidence["process_limits_source"],
        "agent_capability_snapshot"
    );
    let unprivileged_supervisor = repo
        .list_source_status(Some("edge-b"), Some("process_supervisor_policy"))
        .await
        .unwrap();
    assert_eq!(
        unprivileged_supervisor[0].evidence["process_limits_status"],
        "degraded_unprivileged"
    );
}

#[tokio::test]
async fn source_status_enriches_backup_and_update_runtime_readiness() {
    let repo = Repository::Memory(MemoryState::default());
    if let Repository::Memory(memory) = &repo {
        upsert_memory_agent(
            &memory.agents,
            &AgentHello {
                client_id: "edge-a".to_string(),
                process_incarnation_id: uuid::Uuid::new_v4(),
                agent_version: "test".to_string(),
                os_release: "test".to_string(),
                arch: "x86_64".to_string(),
                update_heartbeat: None,
                internal_build_number: 1,
                capabilities: Default::default(),
            },
        )
        .await;
        upsert_memory_agent(
            &memory.agents,
            &AgentHello {
                client_id: "edge-b".to_string(),
                process_incarnation_id: uuid::Uuid::new_v4(),
                agent_version: "test".to_string(),
                os_release: "test".to_string(),
                arch: "x86_64".to_string(),
                update_heartbeat: None,
                internal_build_number: 1,
                capabilities: Default::default(),
            },
        )
        .await;
        let backup_request_id = Uuid::new_v4();
        let restore_plan_id = Uuid::new_v4();
        memory
            .backup_artifacts
            .write()
            .await
            .push(BackupArtifactView {
                id: Uuid::new_v4(),
                client_id: "edge-a".to_string(),
                object_key: "backups/edge-a/artifact.tar".to_string(),
                sha256_hex: "1".repeat(64),
                size_bytes: 4096,
                status: "active".to_string(),
                content_available: true,
                created_at: "100".to_string(),
            });
        memory
            .backup_requests
            .write()
            .await
            .push(BackupRequestView {
                id: backup_request_id,
                actor_id: None,
                client_id: "edge-a".to_string(),
                paths: vec!["/srv/app".to_string()],
                include_config: true,
                follow_symlinks: false,
                missing_path_policy: vpsman_common::BackupMissingPathPolicy::Fail,
                status: "artifact_metadata_recorded".to_string(),
                payload_hash: "6".repeat(64),
                command_scope: "backup".to_string(),
                artifact_id: None,
                source_job_id: Some(Uuid::new_v4()),
                source_schedule_id: None,
                note: None,
                created_at: "100".to_string(),
            });
        memory.restore_plans.write().await.push(RestorePlanView {
            id: restore_plan_id,
            actor_id: None,
            source_backup_request_id: backup_request_id,
            source_client_id: "edge-a".to_string(),
            target_client_id: "edge-b".to_string(),
            paths: vec!["/srv/app".to_string()],
            include_config: true,
            destination_root: Some("/restore".to_string()),
            status: "planned_metadata_only".to_string(),
            payload_hash: "7".repeat(64),
            command_scope: "restore".to_string(),
            note: None,
            created_at: "101".to_string(),
        });
        memory
            .migration_links
            .write()
            .await
            .push(MigrationLinkView {
                id: Uuid::new_v4(),
                actor_id: None,
                restore_plan_id,
                source_backup_request_id: backup_request_id,
                source_client_id: "edge-a".to_string(),
                target_client_id: "edge-b".to_string(),
                paths: vec!["/srv/app".to_string()],
                include_config: true,
                destination_root: Some("/restore".to_string()),
                status: "linked_metadata_only".to_string(),
                note: None,
                created_at: "102".to_string(),
            });
    }
    let tunnel_input = TunnelPlanInput {
        name: "edge-a-b".to_string(),
        interface_name: "tunab".to_string(),
        kind: TunnelKind::Gre,
        runtime_control: RuntimeTunnelControl {
            manager: RuntimeTunnelManager::AgentIproute2Managed,
            traffic_limit: RuntimeTunnelTrafficLimit {
                ingress_kbps: Some(5_000),
                egress_kbps: Some(10_000),
                burst_kb: Some(256),
            },
            ..RuntimeTunnelControl::default()
        },
        runtime_topology: Default::default(),
        left_client_id: "edge-a".to_string(),
        right_client_id: "edge-b".to_string(),
        left_remote_underlay: "198.51.100.10".to_string(),
        right_remote_underlay: "198.51.100.11".to_string(),
        left_local_underlay: None,
        right_local_underlay: None,
        address_pool_cidr: "10.42.0.0/30".to_string(),
        reserved_addresses: Vec::new(),
        ipv4_tunnel: Some(vpsman_common::TunnelAddressPair {
            left: "10.42.0.0".to_string(),
            right: "10.42.0.1".to_string(),
            prefix_len: 31,
        }),
        ipv6_address_pool_cidr: None,
        ipv6_tunnel: None,
        latency_primary_family: Default::default(),
        bandwidth_mbps: 100,
        ospf: Some(vpsman_common::TunnelOspfConfig {
            mode: vpsman_common::OspfControlMode::Reviewed,
            planned_latency_ms: 12.0,
            planned_packet_loss_ratio: 0.0,
            preference: 1.0,
            policy: OspfCostPolicy::default(),
            min_cost_delta: 5,
            healthy_windows: 2,
            left_adapter_template_id: "33333333-3333-4333-8333-333333333333".to_string(),
            right_adapter_template_id: "44444444-4444-4444-8444-444444444444".to_string(),
        }),
    };
    let tunnel_plan = plan_tunnel(&tunnel_input).unwrap();
    repo.record_tunnel_plan(&tunnel_input, &tunnel_plan, true, &memory_admin())
        .await
        .unwrap();
    let observation_job = Uuid::new_v4();
    repo.record_network_observations(
        observation_job,
        "edge-a",
        &[
            CommandOutput {
                job_id: observation_job,
                stream: OutputStream::Status,
                data: serde_json::to_vec(&serde_json::json!({
                    "type": "network_probe",
                    "plan": "edge-a-b",
                    "interface": "tunab",
                    "peer_client_id": "edge-b",
                    "target": "10.42.0.2",
                    "parsed": {
                        "healthy": true,
                        "latency_avg_ms": 11.5,
                        "packet_loss_ratio": 0.0
                    }
                }))
                .unwrap(),
                exit_code: Some(0),
                done: true,
            },
            CommandOutput {
                job_id: observation_job,
                stream: OutputStream::Status,
                data: serde_json::to_vec(&serde_json::json!({
                    "type": "network_speed_test",
                    "role": "client",
                    "plan": "edge-a-b",
                    "interface": "tunab",
                    "peer_client_id": "edge-b",
                    "server_address": "10.42.0.2",
                    "port": 5201,
                    "success": true,
                    "bytes": 1048576,
                    "throughput_mbps": 90.0
                }))
                .unwrap(),
                exit_code: Some(0),
                done: true,
            },
        ],
    )
    .await
    .unwrap();

    let no_store_state = source_template_test_state(repo.clone(), None);
    let no_store_rows = no_store_state
        .list_source_status(Some("edge-a"), None)
        .await
        .unwrap();
    let backup = status_row(&no_store_rows, "backup_object_store");
    assert_eq!(backup.status, "selected_no_store");
    assert_eq!(backup.evidence["server_object_store_configured"], false);
    assert_eq!(backup.evidence["artifact_count"], 1);
    assert_eq!(backup.evidence["backup_request_count"], 1);
    assert_eq!(backup.evidence["restore_source_count"], 1);
    assert_eq!(backup.evidence["migration_source_count"], 1);
    let restore_mapping = status_row(&no_store_rows, "restore_path_mapping");
    assert_eq!(restore_mapping.status, "ready_on_demand");
    assert_eq!(restore_mapping.evidence["restore_source_count"], 1);
    assert_eq!(restore_mapping.evidence["migration_source_count"], 1);
    let update = status_row(&no_store_rows, "update_artifact_source");
    assert_eq!(update.status, "selected_no_artifacts");
    assert_eq!(update.evidence["release_count"], 0);
    let update_restart = status_row(&no_store_rows, "update_restart_policy");
    assert_eq!(update_restart.status, "ready_on_demand");
    let update_heartbeat = status_row(&no_store_rows, "update_rollback_heartbeat_source");
    assert_eq!(update_heartbeat.status, "ready_on_demand");
    let traffic = status_row(&no_store_rows, "runtime_traffic_accounting_source");
    assert_eq!(traffic.evidence["traffic_limit_plan_count"], 1);
    assert_eq!(traffic.evidence["traffic_limit_apply_plan_count"], 1);
    let traffic_limits = status_row(&no_store_rows, "traffic_limit_status_source");
    assert_eq!(traffic_limits.status, "ready");
    assert_eq!(traffic_limits.evidence["traffic_limit_plan_count"], 1);
    assert!(no_store_rows.iter().all(|row| {
        !matches!(
            row.domain.as_str(),
            "runtime_tunnel_adapter" | "routing_cost_adapter"
        )
    }));

    if let Repository::Memory(memory) = &repo {
        memory
            .agent_update_releases
            .write()
            .await
            .push(AgentUpdateReleaseView {
                id: Uuid::new_v4(),
                actor_id: None,
                name: "vpsman-agent".to_string(),
                version: "2.0.0".to_string(),
                channel: "stable".to_string(),
                status: "published_external".to_string(),
                artifact_sha256_hex: "2".repeat(64),
                artifact_url_sha256_hex: Some("5".repeat(64)),
                rollback_artifact_sha256_hex: None,
                rollback_artifact_url_sha256_hex: None,
                rollback_size_bytes: None,
                size_bytes: Some(8192),
                notes: None,
                created_at: "101".to_string(),
            });
    }
    let metadata_only_rows = no_store_state
        .list_source_status(Some("edge-a"), Some("update_artifact_source"))
        .await
        .unwrap();
    let update = status_row(&metadata_only_rows, "update_artifact_source");
    assert_eq!(update.status, "ready");
    assert_eq!(update.evidence["external_release_count"], 1);

    let backup_store_root =
        std::env::temp_dir().join(format!("vpsman-backup-store-{}", Uuid::new_v4()));
    let ready_state = source_template_test_state(
        repo.clone(),
        Some(BackupObjectStore::filesystem(backup_store_root).unwrap()),
    );
    let ready_rows = ready_state
        .list_source_status(Some("edge-a"), None)
        .await
        .unwrap();
    let backup = status_row(&ready_rows, "backup_object_store");
    assert_eq!(backup.status, "ready");
    assert_eq!(backup.evidence["server_object_store_kind"], "filesystem");
    let update = status_row(&ready_rows, "update_artifact_source");
    assert_eq!(update.status, "ready");
    assert_eq!(update.evidence["release_count"], 1);
    assert_eq!(update.evidence["external_release_count"], 1);
}

#[tokio::test]
async fn source_template_assignment_reads_compute_defaults_without_persisting_hidden_clients() {
    let repo = Repository::Memory(MemoryState::default());
    if let Repository::Memory(memory) = &repo {
        for client_id in ["edge-a", "edge-hidden"] {
            upsert_memory_agent(
                &memory.agents,
                &AgentHello {
                    client_id: client_id.to_string(),
                    process_incarnation_id: uuid::Uuid::new_v4(),
                    agent_version: "test".to_string(),
                    os_release: "test".to_string(),
                    arch: "x86_64".to_string(),
                    update_heartbeat: None,
                    internal_build_number: 1,
                    capabilities: Default::default(),
                },
            )
            .await;
        }
        memory
            .hidden_clients
            .write()
            .await
            .insert("edge-hidden".to_string());
    }

    let assignments = repo
        .list_source_template_assignments(None, None)
        .await
        .unwrap();
    assert!(assignments.iter().any(|assignment| {
        assignment.client_id == "edge-a"
            && assignment.domain == "runtime_traffic_accounting_source"
            && assignment.template_name == "builtin:interface_counters"
    }));
    assert!(assignments
        .iter()
        .all(|assignment| assignment.client_id != "edge-hidden"));

    let templates = repo.list_source_templates(None).await.unwrap();
    let default_template = templates
        .iter()
        .find(|template| {
            template.domain == "runtime_traffic_accounting_source"
                && template.name == "builtin:interface_counters"
        })
        .unwrap();
    assert_eq!(default_template.assigned_client_count, 1);

    if let Repository::Memory(memory) = &repo {
        assert!(
            memory.source_template_assignments.read().await.is_empty(),
            "default assignment reads must not persist durable rows"
        );
    }
}

fn source_template_test_state(
    repo: Repository,
    backup_object_store: Option<BackupObjectStore>,
) -> AppState {
    AppState {
        repo,
        events: tokio::sync::broadcast::channel(4).0,
        internal_token: None,
        gateway: GatewayDispatchClient::default(),
        backup_object_store,
        update_release_policy: Default::default(),
        fleet_alert_policy: Default::default(),
        job_output_artifact_min_bytes: 32768,
        artifact_max_bytes: crate::state::DEFAULT_ARTIFACT_MAX_BYTES,
        require_registered_agent_updates: false,
        suite_config_path: std::path::PathBuf::from("config/vpsman.toml"),
        dispatcher_config: crate::state::DispatcherRuntimeConfig::default(),
    }
}

fn status_row<'a>(rows: &'a [SourceStatusView], domain: &str) -> &'a SourceStatusView {
    rows.iter()
        .find(|row| row.domain == domain)
        .unwrap_or_else(|| panic!("missing source template status row for {domain}"))
}

fn memory_admin() -> AuthContext {
    AuthContext {
        operator: OperatorView {
            id: Uuid::nil(),
            username: "test-operator".to_string(),
            role: "admin".to_string(),
            scopes: vec!["*".to_string()],
            preferences: crate::model::OperatorPreferences::default(),
            totp_enabled: false,
            status: "active".to_string(),
            session_refresh_ttl_secs: crate::DEFAULT_REFRESH_TOKEN_TTL_SECS,
            created_at: crate::unix_now().to_string(),
            disabled_at: None,
            deleted_at: None,
        },
        session_id: Uuid::nil(),
    }
}
