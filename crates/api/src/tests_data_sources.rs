use super::*;
use vpsman_common::{
    plan_tunnel, AgentCapabilitySnapshot, AgentHello, AgentPrivilegeMode, BandwidthTier,
    CommandOutput, OspfCostPolicy, OutputStream, RuntimeTunnelCommand, RuntimeTunnelControl,
    RuntimeTunnelManager, RuntimeTunnelTrafficLimit, TunnelKind, TunnelPlanInput,
};

#[tokio::test]
async fn data_source_presets_assign_defaults_and_shared_custom_presets() {
    let repo = Repository::Memory(MemoryState::default());
    if let Repository::Memory(memory) = &repo {
        for client_id in ["client-a", "client-b"] {
            upsert_memory_agent(
                &memory.agents,
                &AgentHello {
                    client_id: client_id.to_string(),
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
        .list_data_source_assignments(Some("client-a"), Some("runtime_traffic_accounting_source"))
        .await
        .unwrap();
    assert_eq!(defaults.len(), 1);
    assert_eq!(defaults[0].preset_name, "builtin:interface_counters");

    let vnstat = repo
        .create_data_source_preset(
            &CreateDataSourcePresetRequest {
                domain: "runtime_traffic_accounting_source".to_string(),
                name: "shared:vnstat-json".to_string(),
                scope: "shared".to_string(),
                owner_client_id: None,
                description: Some("Provider image with vnstat installed".to_string()),
                definition: serde_json::json!({
                    "source": "vnstat",
                    "traffic_command": {
                        "argv": ["/usr/bin/vnstat", "--json"],
                        "timeout_secs": 2,
                        "max_output_bytes": 4096
                    }
                }),
            },
            &operator,
        )
        .await
        .unwrap();

    let preview = repo
        .assign_data_source_preset(
            &AssignDataSourcePresetRequest {
                domain: "runtime_traffic_accounting_source".to_string(),
                preset_id: vnstat.id,
                selector_expression: "id:client-a || id:client-b".to_string(),
                confirmed: false,
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
        .all(|assignment| assignment.preset_name == "builtin:interface_counters"));

    let assigned = repo
        .assign_data_source_preset(
            &AssignDataSourcePresetRequest {
                domain: "runtime_traffic_accounting_source".to_string(),
                preset_id: vnstat.id,
                selector_expression: "id:client-a || id:client-b".to_string(),
                confirmed: true,
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
        .all(|assignment| assignment.preset_name == "shared:vnstat-json"));
    assert!(repo
        .list_audit_logs(10)
        .await
        .unwrap()
        .iter()
        .any(|audit| audit.action == "data_source_preset.assigned"));
}

#[tokio::test]
async fn curated_builtin_data_source_presets_are_selectable_not_default() {
    let repo = Repository::Memory(MemoryState::default());
    if let Repository::Memory(memory) = &repo {
        upsert_memory_agent(
            &memory.agents,
            &AgentHello {
                client_id: "edge-a".to_string(),
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

    let presets = repo.list_data_source_presets(None).await.unwrap();
    let domains = crate::data_source_builtin_presets::DATA_SOURCE_DOMAINS;
    assert!(presets.len() > domains.len());
    for domain in domains {
        let defaults = presets
            .iter()
            .filter(|preset| preset.domain == *domain && preset.is_default)
            .collect::<Vec<_>>();
        assert_eq!(
            defaults.len(),
            1,
            "expected one default preset for {domain}"
        );
    }

    let default_assignments = repo
        .list_data_source_assignments(Some("edge-a"), None)
        .await
        .unwrap();
    assert_eq!(default_assignments.len(), domains.len());
    assert!(default_assignments
        .iter()
        .all(|assignment| assignment.preset_scope == "built_in"));
    assert!(!default_assignments
        .iter()
        .any(|assignment| assignment.preset_name == "builtin:vnstat_json"));

    let curated_names = [
        "builtin:host_mounted_procfs",
        "builtin:vnstat_json",
        "builtin:usr_bin_ping",
        "builtin:usr_bin_w",
        "builtin:busybox_ash_argv",
        "builtin:agent_iproute2_runtime_reconcile",
        "builtin:s3_path_style_reserved",
        "builtin:https_signed_artifact",
    ];
    for name in curated_names {
        let preset = presets
            .iter()
            .find(|preset| preset.name == name)
            .unwrap_or_else(|| panic!("missing curated preset {name}"));
        assert!(preset.built_in);
        assert!(!preset.is_default);
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
        (
            "runtime_tunnel_adapter",
            "builtin:agent_iproute2_runtime_reconcile",
            "runtime_reconcile_enabled = true",
        ),
    ];
    for (domain, preset_name, _) in assignments {
        let preset = presets
            .iter()
            .find(|preset| preset.domain == domain && preset.name == preset_name)
            .unwrap();
        repo.assign_data_source_preset(
            &AssignDataSourcePresetRequest {
                domain: domain.to_string(),
                preset_id: preset.id,
                selector_expression: "id:edge-a".to_string(),
                confirmed: false,
            },
            &operator,
        )
        .await
        .unwrap();
    }

    let rendered = repo.render_data_source_hot_config("edge-a").await.unwrap();
    for (_, _, expected) in assignments {
        assert!(
            rendered.toml.contains(expected),
            "rendered hot config missing {expected}:\n{}",
            rendered.toml
        );
    }

    for preset_name in [
        "builtin:s3_path_style_reserved",
        "builtin:https_signed_artifact",
    ] {
        let preset = presets
            .iter()
            .find(|preset| preset.name == preset_name)
            .unwrap();
        let tested = repo
            .test_data_source_preset(
                preset.id,
                &TestDataSourcePresetRequest {
                    definition: preset.definition.clone(),
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
async fn data_source_preset_lifecycle_updates_the_shared_model() {
    let repo = Repository::Memory(MemoryState::default());
    if let Repository::Memory(memory) = &repo {
        for client_id in ["client-a", "client-b"] {
            upsert_memory_agent(
                &memory.agents,
                &AgentHello {
                    client_id: client_id.to_string(),
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
    let preset = repo
        .create_data_source_preset(
            &CreateDataSourcePresetRequest {
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
    repo.assign_data_source_preset(
        &AssignDataSourcePresetRequest {
            domain: "runtime_traffic_accounting_source".to_string(),
            preset_id: preset.id,
            selector_expression: "id:client-a || id:client-b".to_string(),
            confirmed: true,
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
        .diff_data_source_preset(
            preset.id,
            &DataSourcePresetDiffRequest {
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
        .test_data_source_preset(
            preset.id,
            &TestDataSourcePresetRequest {
                definition: candidate.clone(),
            },
        )
        .await
        .unwrap();
    assert!(test.valid);
    assert!(test.renderable);
    assert!(test.toml.contains("runtime_vnstat_argv"));

    let preview = repo
        .update_data_source_preset(
            preset.id,
            &UpdateDataSourcePresetRequest {
                description: Some("provider image uses vnstat".to_string()),
                definition: candidate.clone(),
                confirmed: false,
                keep_description: false,
            },
            &operator,
        )
        .await
        .unwrap();
    assert!(preview.confirmation_required);
    assert_eq!(preview.affected_client_count, 2);
    assert_eq!(
        preview.preset.definition,
        serde_json::json!({"source": "interface_counters"})
    );

    let updated = repo
        .update_data_source_preset(
            preset.id,
            &UpdateDataSourcePresetRequest {
                description: Some("provider image uses vnstat".to_string()),
                definition: candidate.clone(),
                confirmed: true,
                keep_description: false,
            },
            &operator,
        )
        .await
        .unwrap();
    assert!(!updated.confirmation_required);
    assert_eq!(updated.preset.definition, candidate);
    assert_eq!(updated.preset.assigned_client_count, 2);
    assert!(repo
        .list_audit_logs(10)
        .await
        .unwrap()
        .iter()
        .any(|audit| audit.action == "data_source_preset.updated"));
}

#[tokio::test]
async fn data_source_preset_clone_keeps_assignment_separate() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = memory_admin();
    let builtins = repo
        .list_data_source_presets(Some("runtime_tunnel_adapter"))
        .await
        .unwrap();
    let source = builtins
        .iter()
        .find(|preset| preset.name == "builtin:agent_iproute2_managed")
        .unwrap();
    let clone = repo
        .clone_data_source_preset(
            source.id,
            &CloneDataSourcePresetRequest {
                name: "shared:iproute2-managed-runtime".to_string(),
                scope: "shared".to_string(),
                owner_client_id: None,
                description: Some("site default runtime tunnel adapter".to_string()),
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
        .any(|audit| audit.action == "data_source_preset.cloned"));
}

#[tokio::test]
async fn vps_local_data_source_preset_only_assigns_to_owner() {
    let repo = Repository::Memory(MemoryState::default());
    if let Repository::Memory(memory) = &repo {
        for client_id in ["client-a", "client-b"] {
            upsert_memory_agent(
                &memory.agents,
                &AgentHello {
                    client_id: client_id.to_string(),
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
    let preset = repo
        .create_data_source_preset(
            &CreateDataSourcePresetRequest {
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
        .assign_data_source_preset(
            &AssignDataSourcePresetRequest {
                domain: "process_inventory_source".to_string(),
                preset_id: preset.id,
                selector_expression: "id:client-b".to_string(),
                confirmed: true,
            },
            &operator,
        )
        .await
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("vps_local_preset_owner_mismatch"));

    let assigned = repo
        .assign_data_source_preset(
            &AssignDataSourcePresetRequest {
                domain: "process_inventory_source".to_string(),
                preset_id: preset.id,
                selector_expression: "id:client-a".to_string(),
                confirmed: false,
            },
            &operator,
        )
        .await
        .unwrap();
    assert_eq!(assigned.assignments[0].client_id, "client-a");
    assert_eq!(
        assigned.assignments[0].preset_name,
        "local:mounted-host-proc"
    );
}

#[tokio::test]
async fn data_source_hot_config_renders_selected_presets() {
    let repo = Repository::Memory(MemoryState::default());
    if let Repository::Memory(memory) = &repo {
        upsert_memory_agent(
            &memory.agents,
            &AgentHello {
                client_id: "edge-a".to_string(),
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
        .create_data_source_preset(
            &CreateDataSourcePresetRequest {
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
                        "timeout_secs": 3,
                        "max_output_bytes": 4096
                    }
                }),
            },
            &operator,
        )
        .await
        .unwrap();
    let process = repo
        .create_data_source_preset(
            &CreateDataSourcePresetRequest {
                domain: "process_inventory_source".to_string(),
                name: "shared:processctl".to_string(),
                scope: "shared".to_string(),
                owner_client_id: None,
                description: None,
                definition: serde_json::json!({
                    "source": "custom_command",
                    "process_inventory_command": {
                        "argv": ["/opt/vpsman/process-inventory"],
                        "timeout_secs": 5,
                        "max_output_bytes": 8192
                    }
                }),
            },
            &operator,
        )
        .await
        .unwrap();
    let vnstat = repo
        .create_data_source_preset(
            &CreateDataSourcePresetRequest {
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
        .create_data_source_preset(
            &CreateDataSourcePresetRequest {
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

    for (domain, preset_id) in [
        ("telemetry_metrics_source", telemetry.id),
        ("process_inventory_source", process.id),
        ("runtime_traffic_accounting_source", vnstat.id),
        ("command_execution_policy", execution.id),
    ] {
        repo.assign_data_source_preset(
            &AssignDataSourcePresetRequest {
                domain: domain.to_string(),
                preset_id,
                selector_expression: "id:edge-a".to_string(),
                confirmed: false,
            },
            &operator,
        )
        .await
        .unwrap();
    }

    let rendered = repo.render_data_source_hot_config("edge-a").await.unwrap();
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
async fn data_source_hot_config_rejects_unsafe_migrated_preset_commands() {
    let repo = Repository::Memory(MemoryState::default());
    if let Repository::Memory(memory) = &repo {
        upsert_memory_agent(
            &memory.agents,
            &AgentHello {
                client_id: "edge-a".to_string(),
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
            .data_source_presets
            .write()
            .await
            .push(DataSourcePresetView {
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
        let preset_id = memory.data_source_presets.read().await.last().unwrap().id;
        memory
            .data_source_assignments
            .write()
            .await
            .push(DataSourcePresetAssignmentView {
                client_id: "edge-a".to_string(),
                domain: "command_execution_policy".to_string(),
                preset_id,
                preset_name: "shared:bad-shell".to_string(),
                preset_scope: "shared".to_string(),
                assigned_at: "0".to_string(),
            });
    }

    let error = repo
        .render_data_source_hot_config("edge-a")
        .await
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("shell_script_argv_executable_must_be_absolute"));
}

#[tokio::test]
async fn data_source_status_links_selected_presets_to_live_source_evidence() {
    let repo = Repository::Memory(MemoryState::default());
    if let Repository::Memory(memory) = &repo {
        upsert_memory_agent(
            &memory.agents,
            &AgentHello {
                client_id: "edge-a".to_string(),
                agent_version: "test".to_string(),
                os_release: "test".to_string(),
                arch: "x86_64".to_string(),
                update_heartbeat: None,
                internal_build_number: 1,
                capabilities: AgentCapabilitySnapshot {
                    privilege_mode: AgentPrivilegeMode::Root,
                    effective_uid: Some(0),
                    can_attempt_privileged_ops: true,
                    can_manage_runtime_tunnels: true,
                    can_apply_process_limits: true,
                    unprivileged_hint: None,
                },
            },
        )
        .await;
        upsert_memory_agent(
            &memory.agents,
            &AgentHello {
                client_id: "edge-b".to_string(),
                agent_version: "test".to_string(),
                os_release: "test".to_string(),
                arch: "x86_64".to_string(),
                update_heartbeat: None,
                internal_build_number: 1,
                capabilities: AgentCapabilitySnapshot {
                    privilege_mode: AgentPrivilegeMode::Unprivileged,
                    effective_uid: Some(1000),
                    can_attempt_privileged_ops: true,
                    can_manage_runtime_tunnels: false,
                    can_apply_process_limits: false,
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
                promotion_required: false,
                plan_correlation: "managed_desired".to_string(),
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
            });
    }
    let operator = memory_admin();
    let vnstat = repo
        .create_data_source_preset(
            &CreateDataSourcePresetRequest {
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
    repo.assign_data_source_preset(
        &AssignDataSourcePresetRequest {
            domain: "runtime_traffic_accounting_source".to_string(),
            preset_id: vnstat.id,
            selector_expression: "id:edge-a".to_string(),
            confirmed: false,
        },
        &operator,
    )
    .await
    .unwrap();

    let all = repo
        .list_data_source_status(Some("edge-a"), None)
        .await
        .unwrap();
    assert_eq!(
        all.len(),
        crate::data_source_builtin_presets::DATA_SOURCE_DOMAINS.len()
    );
    assert!(all
        .iter()
        .any(|row| row.domain == "telemetry_metrics_source"
            && row.preset_name == "builtin:linux_procfs"
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
        .list_data_source_status(Some("edge-a"), Some("runtime_traffic_accounting_source"))
        .await
        .unwrap();
    assert_eq!(traffic.len(), 1);
    assert_eq!(traffic[0].preset_name, "shared:vnstat");
    assert_eq!(traffic[0].source_kind, "vnstat");
    assert_eq!(traffic[0].status, "ok");
    assert_eq!(traffic[0].evidence["sample_count"], 1);

    let tunnels = repo
        .list_data_source_status(Some("edge-a"), Some("runtime_tunnel_adapter"))
        .await
        .unwrap();
    assert_eq!(tunnels.len(), 1);
    assert_eq!(tunnels[0].status, "ok");
    assert_eq!(
        tunnels[0].evidence["samples"][0]["plan_correlation"],
        "telemetry_reported_plan"
    );

    let unprivileged_process = repo
        .list_data_source_status(Some("edge-b"), Some("process_inventory_source"))
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
        .list_data_source_status(Some("edge-b"), Some("process_supervisor_policy"))
        .await
        .unwrap();
    assert_eq!(
        unprivileged_supervisor[0].evidence["process_limits_status"],
        "degraded_unprivileged"
    );
}

#[tokio::test]
async fn data_source_status_enriches_backup_and_update_runtime_readiness() {
    let repo = Repository::Memory(MemoryState::default());
    if let Repository::Memory(memory) = &repo {
        upsert_memory_agent(
            &memory.agents,
            &AgentHello {
                client_id: "edge-a".to_string(),
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
                object_key: "backups/edge-a/artifact.json".to_string(),
                sha256_hex: "1".repeat(64),
                encrypted: true,
                size_bytes: 4096,
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
            manager: RuntimeTunnelManager::ExternalManagedAdapter,
            traffic_limit_apply: Some(RuntimeTunnelCommand {
                argv: vec!["/bin/true".to_string()],
                ..RuntimeTunnelCommand::default()
            }),
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
        left_underlay: "198.51.100.10".to_string(),
        right_underlay: "198.51.100.11".to_string(),
        address_pool_cidr: "10.42.0.0/30".to_string(),
        reserved_addresses: Vec::new(),
        bandwidth: BandwidthTier::M100,
        latency_ms: 12.0,
        packet_loss_ratio: 0.0,
        preference: 1.0,
        ospf_policy: OspfCostPolicy::default(),
    };
    let tunnel_plan = plan_tunnel(&tunnel_input).unwrap();
    repo.record_tunnel_plan(&tunnel_input, &tunnel_plan, &memory_admin())
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

    let no_store_state = data_source_test_state(repo.clone(), None, None);
    let no_store_rows = no_store_state
        .list_data_source_status(Some("edge-a"), None)
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
    let tunnel = status_row(&no_store_rows, "runtime_tunnel_adapter");
    assert_eq!(tunnel.evidence["routing_recommendation_count"], 1);
    assert_eq!(tunnel.evidence["probe_sample_count"], 1);
    assert_eq!(tunnel.evidence["speed_sample_count"], 1);
    let routing = status_row(&no_store_rows, "routing_daemon_adapter");
    assert_eq!(routing.status, "ready");
    assert_eq!(routing.evidence["routing_recommendation_count"], 1);

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
                status: "published_metadata_only".to_string(),
                artifact_sha256_hex: "2".repeat(64),
                artifact_signature_provided: true,
                artifact_signature_sha256_hex: Some("3".repeat(64)),
                artifact_signing_key_sha256_hex: "4".repeat(64),
                artifact_url_sha256_hex: Some("5".repeat(64)),
                artifact_object_key: None,
                artifact_download_path: None,
                artifact_download_url: None,
                rollback_artifact_sha256_hex: None,
                rollback_artifact_signature_provided: false,
                rollback_artifact_signature_sha256_hex: None,
                rollback_artifact_signing_key_sha256_hex: None,
                rollback_artifact_url_sha256_hex: None,
                rollback_artifact_object_key: None,
                rollback_artifact_download_path: None,
                rollback_artifact_download_url: None,
                rollback_size_bytes: None,
                size_bytes: Some(8192),
                notes: None,
                created_at: "101".to_string(),
            });
    }
    let metadata_only_rows = no_store_state
        .list_data_source_status(Some("edge-a"), Some("update_artifact_source"))
        .await
        .unwrap();
    let update = status_row(&metadata_only_rows, "update_artifact_source");
    assert_eq!(update.status, "metadata_only");
    assert_eq!(update.evidence["external_release_count"], 1);

    let backup_store_root =
        std::env::temp_dir().join(format!("vpsman-backup-store-{}", Uuid::new_v4()));
    let update_store_root =
        std::env::temp_dir().join(format!("vpsman-update-store-{}", Uuid::new_v4()));
    let ready_state = data_source_test_state(
        repo.clone(),
        Some(BackupObjectStore::filesystem(backup_store_root).unwrap()),
        Some(BackupObjectStore::filesystem(update_store_root).unwrap()),
    );
    let ready_rows = ready_state
        .list_data_source_status(Some("edge-a"), None)
        .await
        .unwrap();
    let backup = status_row(&ready_rows, "backup_object_store");
    assert_eq!(backup.status, "ready");
    assert_eq!(backup.evidence["server_object_store_kind"], "filesystem");
    let update = status_row(&ready_rows, "update_artifact_source");
    assert_eq!(update.status, "ready");
    assert_eq!(update.evidence["server_object_store_kind"], "filesystem");
    assert_eq!(update.evidence["release_count"], 1);
}

fn data_source_test_state(
    repo: Repository,
    backup_object_store: Option<BackupObjectStore>,
    update_object_store: Option<BackupObjectStore>,
) -> AppState {
    AppState {
        repo,
        events: tokio::sync::broadcast::channel(4).0,
        internal_token: None,
        gateway: GatewayDispatchClient::default(),
        backup_object_store,
        update_object_store,
        update_artifact_public_base_url: None,
        update_release_policy: Default::default(),
        fleet_alert_policy: Default::default(),
        job_output_artifact_min_bytes: 32768,
        require_registered_agent_updates: false,
    }
}

fn status_row<'a>(rows: &'a [DataSourceStatusView], domain: &str) -> &'a DataSourceStatusView {
    rows.iter()
        .find(|row| row.domain == domain)
        .unwrap_or_else(|| panic!("missing data-source status row for {domain}"))
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
        },
        session_id: Uuid::nil(),
    }
}
