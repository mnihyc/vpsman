use super::*;
use crate::{model::AgentView, repository::MemoryState};

#[test]
fn all_default_presets_render_as_one_agent_runtime_config_without_nulls() {
    let defaults = system_configuration_presets()
        .into_iter()
        .filter(|preset| preset.is_default)
        .collect::<Vec<_>>();
    assert_eq!(defaults.len(), CONFIGURATION_BEHAVIORS.len());

    let mut sections = serde_json::json!({"version": 1});
    for preset in defaults {
        let rendered =
            render_configuration_preset_definition(preset.behavior, &preset.definition).unwrap();
        assert!(
            !contains_json_null(&rendered.sections),
            "{} rendered a JSON null",
            preset.behavior
        );
        merge_json_object(&mut sections, rendered.sections).unwrap();
    }

    let _: vpsman_common::AgentRuntimeConfig = serde_json::from_value(sections.clone()).unwrap();
    let toml = toml::to_string_pretty(&sections).unwrap();
    let _: vpsman_common::AgentRuntimeConfig = toml::from_str(&toml).unwrap();
}

#[test]
fn operator_visible_names_reject_control_characters() {
    assert!(validate_operator_name("Daily checks", "invalid").is_ok());
    assert!(validate_operator_name("Daily\nchecks", "invalid").is_err());
    assert!(validate_operator_name("\t", "invalid").is_err());
}

#[test]
fn preset_definitions_reject_missing_discriminators_and_unknown_fields() {
    assert!(
        validate_configuration_preset_definition("latency_probe", &serde_json::json!({})).is_err()
    );
    assert!(validate_configuration_preset_definition(
        "latency_probe",
        &serde_json::json!({
            "source": "interface_counters",
            "unexpected": true
        })
    )
    .is_err());
    assert!(validate_configuration_preset_definition(
        "latency_probe",
        &serde_json::json!({
            "source": "linux_ping_preset",
            "unexpected": true
        })
    )
    .is_err());
    assert!(validate_configuration_preset_definition(
        "user_sessions",
        &serde_json::json!({
            "source": "linux_w_who_preset",
            "unexpected": true
        })
    )
    .is_err());
}

#[test]
fn ospf_updater_presets_are_explicitly_unconfigured_or_fully_paired() {
    let default = system_configuration_presets()
        .into_iter()
        .find(|preset| preset.behavior == "ospf_update_command" && preset.is_default)
        .unwrap();
    assert!(parse_ospf_update_commands(&default.definition)
        .unwrap()
        .is_none());

    let configured = serde_json::json!({
        "contract_version": vpsman_common::ROUTING_COST_ADAPTER_CONTRACT_VERSION,
        "status_command": preset_command("/usr/bin/ospf-status"),
        "update_command": preset_command("/usr/bin/ospf-update")
    });
    let rendered =
        render_configuration_preset_definition("ospf_update_command", &configured).unwrap();
    let mut sections = serde_json::json!({"version": 1});
    merge_json_object(&mut sections, rendered.sections).unwrap();
    let runtime: vpsman_common::AgentRuntimeConfig = serde_json::from_value(sections).unwrap();
    assert_eq!(
        runtime.network.ospf_status_command.unwrap().argv,
        ["/usr/bin/ospf-status"]
    );
    assert_eq!(
        runtime.network.ospf_update_command.unwrap().argv,
        ["/usr/bin/ospf-update"]
    );

    let mut legacy = configured.clone();
    legacy["contract_version"] = serde_json::json!(1);
    assert!(validate_configuration_preset_definition("ospf_update_command", &legacy).is_err());

    assert!(validate_configuration_preset_definition(
        "ospf_update_command",
        &serde_json::json!({
            "contract_version": vpsman_common::ROUTING_COST_ADAPTER_CONTRACT_VERSION,
            "status_command": preset_command("/usr/bin/ospf-status"),
            "update_command": null
        })
    )
    .is_err());
}

#[test]
fn preset_paths_reject_controls_and_unbounded_values() {
    assert!(validate_absolute_path("/proc", "proc_root").is_ok());
    assert!(validate_absolute_path("/proc\n", "proc_root").is_err());
    assert!(
        validate_absolute_path(&format!("/{}", "a".repeat(MAX_ARG_BYTES)), "proc_root").is_err()
    );
}

#[tokio::test]
async fn override_apply_rejects_a_changed_selection_origin() {
    let memory = MemoryState::default();
    memory.agents.write().await.push(test_agent("edge-a"));
    let repo = Repository::Memory(memory);
    let operator = crate::tests::test_operator();
    let preset = create_test_latency_preset(&repo, "vnStat A", "/usr/bin/vnstat", &operator).await;
    let preview = repo
        .preview_configuration_source_override(&PreviewConfigurationSourceOverrideRequest {
            action: ConfigurationOverrideAction::Set,
            behavior: "latency_probe".to_string(),
            preset_id: Some(preset.id),
            selector_expression: String::new(),
            target_client_ids: vec!["edge-a".to_string()],
        })
        .await
        .unwrap();
    let Repository::Memory(memory) = &repo else {
        unreachable!()
    };
    memory
        .configuration_preset_overrides
        .write()
        .await
        .push(ConfigurationPresetOverrideRecord {
            client_id: "edge-a".to_string(),
            behavior: "latency_probe".to_string(),
            preset_id: preset.id,
            updated_at: unix_now().to_string(),
        });

    let error = repo
        .apply_configuration_source_override(&preview, &operator)
        .await
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("configuration_source_override_preview_stale"));
}

#[tokio::test]
async fn override_preview_and_audit_retain_the_trimmed_selector() {
    let memory = MemoryState::default();
    memory.agents.write().await.push(test_agent("edge-a"));
    let repo = Repository::Memory(memory);
    let operator = crate::tests::test_operator();
    let preset =
        create_test_latency_preset(&repo, "vnStat selector", "/usr/bin/vnstat", &operator).await;
    let preview = repo
        .preview_configuration_source_override(&PreviewConfigurationSourceOverrideRequest {
            action: ConfigurationOverrideAction::Set,
            behavior: "latency_probe".to_string(),
            preset_id: Some(preset.id),
            selector_expression: "  tag:edge  ".to_string(),
            target_client_ids: vec!["edge-a".to_string()],
        })
        .await
        .unwrap();
    assert_eq!(preview.selector_expression, "tag:edge");

    repo.apply_configuration_source_override(&preview, &operator)
        .await
        .unwrap();
    let Repository::Memory(memory) = &repo else {
        unreachable!()
    };
    let audits = memory.audits.read().await;
    let applied = audits
        .iter()
        .find(|entry| entry.action == "configuration_source_override.applied")
        .unwrap();
    assert_eq!(applied.metadata["selector_expression"], "tag:edge");
}

#[tokio::test]
async fn deleting_agent_hides_but_preserves_its_configuration_preset_override() {
    let memory = MemoryState::default();
    memory.agents.write().await.push(test_agent("edge-delete"));
    let repo = Repository::Memory(memory);
    let operator = crate::tests::test_operator();
    let preset =
        create_test_latency_preset(&repo, "Retired edge traffic", "/usr/bin/vnstat", &operator)
            .await;
    let preview = repo
        .preview_configuration_source_override(&PreviewConfigurationSourceOverrideRequest {
            action: ConfigurationOverrideAction::Set,
            behavior: "latency_probe".to_string(),
            preset_id: Some(preset.id),
            selector_expression: String::new(),
            target_client_ids: vec!["edge-delete".to_string()],
        })
        .await
        .unwrap();
    repo.apply_configuration_source_override(&preview, &operator)
        .await
        .unwrap();
    let assigned = repo
        .list_configuration_presets(Some("latency_probe"))
        .await
        .unwrap()
        .into_iter()
        .find(|candidate| candidate.id == preset.id)
        .unwrap();
    assert_eq!(assigned.override_vps_count, 1);

    repo.delete_agent("edge-delete", Some("retired"), &operator)
        .await
        .unwrap();

    let released = repo
        .list_configuration_presets(Some("latency_probe"))
        .await
        .unwrap()
        .into_iter()
        .find(|candidate| candidate.id == preset.id)
        .unwrap();
    assert_eq!(released.override_vps_count, 0);
    assert_eq!(released.effective_vps_count, 0);
    let update_preview = repo
        .preview_configuration_preset_update(
            preset.id,
            &PreviewConfigurationPresetRequest {
                description: Some("Updated after endpoint retirement".to_string()),
                definition: serde_json::json!({
                    "source": "configured_ping_argv",
                    "probe_ping_argv": ["/opt/vnstat"]
                }),
            },
        )
        .await
        .unwrap();
    assert!(update_preview.affected_client_ids.is_empty());
    let updated = repo
        .update_configuration_preset(preset.id, &update_preview, &operator)
        .await
        .unwrap();
    assert_eq!(updated.override_vps_count, 0);
    assert_eq!(updated.effective_vps_count, 0);
    let stale_error = repo
        .apply_configuration_source_override(&preview, &operator)
        .await
        .unwrap_err();
    assert!(stale_error
        .to_string()
        .contains("configuration_source_override_preview_stale"));
    let Repository::Memory(memory) = &repo else {
        unreachable!()
    };
    assert_eq!(memory.configuration_preset_overrides.read().await.len(), 1);
    repo.delete_configuration_preset(preset.id, &operator)
        .await
        .unwrap();
    assert!(memory
        .configuration_preset_overrides
        .read()
        .await
        .is_empty());
    assert!(repo
        .configuration_preset_by_id(preset.id)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn revoking_agent_key_keeps_its_configuration_preset_override() {
    let memory = MemoryState::default();
    memory.agents.write().await.push(test_agent("edge-revoke"));
    memory
        .client_public_keys
        .write()
        .await
        .insert("edge-revoke".to_string(), vec![0x42; 32]);
    let repo = Repository::Memory(memory);
    let operator = crate::tests::test_operator();
    let preset =
        create_test_latency_preset(&repo, "Revoked edge traffic", "/usr/bin/vnstat", &operator)
            .await;
    let preview = repo
        .preview_configuration_source_override(&PreviewConfigurationSourceOverrideRequest {
            action: ConfigurationOverrideAction::Set,
            behavior: "latency_probe".to_string(),
            preset_id: Some(preset.id),
            selector_expression: String::new(),
            target_client_ids: vec!["edge-revoke".to_string()],
        })
        .await
        .unwrap();
    repo.apply_configuration_source_override(&preview, &operator)
        .await
        .unwrap();

    repo.revoke_current_client_key("edge-revoke", Some("compromised"), &operator)
        .await
        .unwrap();

    let Repository::Memory(memory) = &repo else {
        unreachable!()
    };
    assert_eq!(memory.configuration_preset_overrides.read().await.len(), 1);
    assert_eq!(
        repo.agent_by_id("edge-revoke").await.unwrap().status,
        "revoked"
    );
}

#[tokio::test]
async fn existing_key_revocation_record_preserves_configuration_override() {
    let memory = MemoryState::default();
    let public_key = vec![0x43; 32];
    memory.agents.write().await.push(test_agent("edge-recover"));
    memory
        .client_public_keys
        .write()
        .await
        .insert("edge-recover".to_string(), public_key.clone());
    memory
        .client_key_revocations
        .write()
        .await
        .push(crate::model::ClientKeyRevocationView {
            id: Uuid::new_v4(),
            client_id: "edge-recover".to_string(),
            public_key_sha256_hex: crate::repository_key_lifecycle::public_key_sha256_hex(
                &public_key,
            ),
            reason: Some("existing record".to_string()),
            revoked_by: Some(Uuid::nil()),
            created_at: unix_now().to_string(),
        });
    memory
        .configuration_preset_overrides
        .write()
        .await
        .push(ConfigurationPresetOverrideRecord {
            client_id: "edge-recover".to_string(),
            behavior: "latency_probe".to_string(),
            preset_id: Uuid::new_v4(),
            updated_at: unix_now().to_string(),
        });
    let repo = Repository::Memory(memory);
    let operator = crate::tests::test_operator();

    repo.revoke_current_client_key("edge-recover", Some("retry"), &operator)
        .await
        .unwrap();

    let Repository::Memory(memory) = &repo else {
        unreachable!()
    };
    assert_eq!(memory.configuration_preset_overrides.read().await.len(), 1);
    let audits = memory.audits.read().await;
    let recovery = audits
        .iter()
        .find(|entry| entry.action == "client_key.revoked")
        .unwrap();
    assert_eq!(recovery.metadata["recovered_existing_revocation"], true);
    assert_eq!(
        repo.agent_by_id("edge-recover").await.unwrap().status,
        "revoked"
    );
}

#[tokio::test]
async fn targeted_source_reads_ignore_unrequested_client_overrides() {
    let memory = MemoryState::default();
    memory
        .agents
        .write()
        .await
        .extend([test_agent("edge-a"), test_agent("edge-b")]);
    memory
        .configuration_preset_overrides
        .write()
        .await
        .push(ConfigurationPresetOverrideRecord {
            client_id: "edge-b".to_string(),
            behavior: "latency_probe".to_string(),
            preset_id: Uuid::new_v4(),
            updated_at: unix_now().to_string(),
        });
    let repo = Repository::Memory(memory);

    let rows = repo
        .list_configuration_sources_for_clients(&["edge-a".to_string()], "latency_probe")
        .await
        .unwrap();

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].client_id, "edge-a");
    assert_eq!(rows[0].selection_origin, "system_default");
}

#[tokio::test]
async fn preset_update_rejects_changed_affected_client_membership() {
    let memory = MemoryState::default();
    memory
        .agents
        .write()
        .await
        .extend([test_agent("edge-a"), test_agent("edge-b")]);
    let repo = Repository::Memory(memory);
    let operator = crate::tests::test_operator();
    let preset = create_test_latency_preset(&repo, "vnStat B", "/usr/bin/vnstat", &operator).await;
    let Repository::Memory(memory) = &repo else {
        unreachable!()
    };
    memory
        .configuration_preset_overrides
        .write()
        .await
        .push(ConfigurationPresetOverrideRecord {
            client_id: "edge-a".to_string(),
            behavior: "latency_probe".to_string(),
            preset_id: preset.id,
            updated_at: unix_now().to_string(),
        });
    let preview = repo
        .preview_configuration_preset_update(
            preset.id,
            &PreviewConfigurationPresetRequest {
                description: preset.description.clone(),
                definition: serde_json::json!({
                    "source": "configured_ping_argv",
                    "probe_ping_argv": ["/opt/vnstat"]
                }),
            },
        )
        .await
        .unwrap();
    assert_eq!(preview.affected_client_ids, vec!["edge-a".to_string()]);
    memory
        .configuration_preset_overrides
        .write()
        .await
        .push(ConfigurationPresetOverrideRecord {
            client_id: "edge-b".to_string(),
            behavior: "latency_probe".to_string(),
            preset_id: preset.id,
            updated_at: unix_now().to_string(),
        });

    let error = repo
        .update_configuration_preset(preset.id, &preview, &operator)
        .await
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("configuration_preset_preview_stale"));
}

#[tokio::test]
async fn adapter_names_are_case_insensitive_and_kind_is_immutable() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = crate::tests::test_operator();
    let created = repo
        .create_network_adapter_definition(&runtime_adapter_request("WireGuard"), &operator)
        .await
        .unwrap();

    let duplicate = repo
        .create_network_adapter_definition(&runtime_adapter_request("wireguard"), &operator)
        .await
        .unwrap_err();
    assert!(duplicate
        .to_string()
        .contains("network_adapter_definition_duplicate"));

    let kind_change = repo
        .update_network_adapter_definition(
            created.id,
            &UpsertNetworkAdapterDefinitionRequest {
                adapter_kind: "routing_cost".to_string(),
                name: created.name,
                description: None,
                definition: serde_json::json!({
                    "contract_version": vpsman_common::ROUTING_COST_ADAPTER_CONTRACT_VERSION,
                    "status_command": preset_command("/usr/bin/status"),
                    "update_command": preset_command("/usr/bin/update")
                }),
            },
            &operator,
        )
        .await
        .unwrap_err();
    assert!(kind_change
        .to_string()
        .contains("network_adapter_definition_kind_immutable"));
}

#[tokio::test]
async fn retired_tunnel_plan_releases_its_adapter_definitions() {
    let memory = MemoryState::default();
    memory
        .agents
        .write()
        .await
        .extend([test_agent("client-a"), test_agent("client-b")]);
    let repo = Repository::Memory(memory);
    let operator = crate::tests::test_operator();
    let left = repo
        .create_network_adapter_definition(&runtime_adapter_request("Runtime left"), &operator)
        .await
        .unwrap();
    let right = repo
        .create_network_adapter_definition(&runtime_adapter_request("Runtime right"), &operator)
        .await
        .unwrap();
    let mut input = crate::tests_network::test_plan_input(
        vpsman_common::RuntimeTunnelManager::CustomAdapter,
        false,
    );
    input.runtime_control.left_adapter_definition_id = Some(left.id.to_string());
    input.runtime_control.right_adapter_definition_id = Some(right.id.to_string());
    let plan = vpsman_common::plan_tunnel(&input).unwrap();
    let saved = repo
        .record_tunnel_plan(&input, &plan, false, &operator)
        .await
        .unwrap();
    repo.delete_tunnel_plan(saved.id, saved.revision, &operator)
        .await
        .unwrap();

    repo.delete_network_adapter_definition(left.id, &operator)
        .await
        .unwrap();
    assert!(repo
        .network_adapter_definition_by_id(left.id, None)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn tunnel_plan_persistence_rejects_missing_adapter_definitions() {
    let memory = MemoryState::default();
    memory
        .agents
        .write()
        .await
        .extend([test_agent("client-a"), test_agent("client-b")]);
    let repo = Repository::Memory(memory);
    let input = crate::tests_network::test_plan_input(
        vpsman_common::RuntimeTunnelManager::CustomAdapter,
        false,
    );
    let plan = vpsman_common::plan_tunnel(&input).unwrap();

    let error = repo
        .record_tunnel_plan(&input, &plan, false, &crate::tests::test_operator())
        .await
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("tunnel_plan_adapter_definition_unavailable"));
}

async fn create_test_latency_preset(
    repo: &Repository,
    name: &str,
    executable: &str,
    operator: &AuthContext,
) -> ConfigurationPresetView {
    repo.create_configuration_preset(
        &CreateConfigurationPresetRequest {
            behavior: "latency_probe".to_string(),
            name: name.to_string(),
            description: None,
            definition: serde_json::json!({
                "source": "configured_ping_argv",
                "probe_ping_argv": [executable]
            }),
        },
        operator,
    )
    .await
    .unwrap()
}

fn runtime_adapter_request(name: &str) -> UpsertNetworkAdapterDefinitionRequest {
    UpsertNetworkAdapterDefinitionRequest {
        adapter_kind: "runtime_tunnel".to_string(),
        name: name.to_string(),
        description: None,
        definition: serde_json::json!({
            "manager": "custom_adapter",
            "contract_version": 1,
            "startup_command": preset_command("/usr/bin/start"),
            "cleanup_command": preset_command("/usr/bin/cleanup"),
            "status_command": preset_command("/usr/bin/status")
        }),
    }
}

fn preset_command(executable: &str) -> Value {
    serde_json::json!({
        "argv": [executable],
        "max_timeout_secs": 10,
        "max_output_bytes": 16384
    })
}

fn test_agent(client_id: &str) -> AgentView {
    AgentView {
        id: client_id.to_string(),
        display_name: client_id.to_string(),
        status: "online".to_string(),
        tags: Vec::new(),
        registration_ip: None,
        last_ip: None,
        last_seen_at: None,
        arch: None,
        internal_build_number: 1,
        process_incarnation_id: None,
        stale_since: None,
        stale_reason: None,
        capabilities: vpsman_common::AgentCapabilitySnapshot::default(),
    }
}

fn contains_json_null(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::Array(values) => values.iter().any(contains_json_null),
        Value::Object(values) => values.values().any(contains_json_null),
        _ => false,
    }
}
