use super::*;

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
            "source": "unsupported_source",
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
fn runtime_command_presets_reject_removed_inventory_placeholders() {
    for placeholder in ["{display_name}", "{tags_csv}"] {
        let command = serde_json::json!({
            "argv": ["/bin/echo", placeholder],
            "max_timeout_secs": 10,
            "max_output_bytes": 16_384
        });
        for (behavior, definition) in [
            (
                "host_metrics",
                serde_json::json!({
                    "source": "custom_command",
                    "custom_metrics_command": command.clone()
                }),
            ),
            (
                "process_inventory",
                serde_json::json!({
                    "source": "custom_command",
                    "process_inventory_command": command.clone()
                }),
            ),
        ] {
            let error =
                validate_configuration_preset_definition(behavior, &definition).unwrap_err();
            assert!(error.to_string().contains("removed_inventory_placeholder"));
        }
    }
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

fn preset_command(executable: &str) -> Value {
    serde_json::json!({
        "argv": [executable],
        "max_timeout_secs": 10,
        "max_output_bytes": 16384
    })
}

fn contains_json_null(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::Array(values) => values.iter().any(contains_json_null),
        Value::Object(values) => values.values().any(contains_json_null),
        _ => false,
    }
}
