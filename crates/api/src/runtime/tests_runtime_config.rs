use super::{
    clear_runtime_tunnel_credentials, operator_dispatch_error, redact_runtime_tunnel_credentials,
    runtime_config_reload_reason, runtime_config_version_after,
};
use crate::error::ApiError;
use vpsman_common::{AgentConfig, RuntimeConfigReconcileResource, RuntimeConfigReconcileScope};

fn scope(
    authoritative: bool,
    resources: &[RuntimeConfigReconcileResource],
) -> RuntimeConfigReconcileScope {
    RuntimeConfigReconcileScope {
        authoritative,
        resources: resources.iter().copied().collect(),
    }
}

#[test]
fn runtime_config_versions_are_strictly_monotonic_above_the_persisted_floor() {
    let first = runtime_config_version_after(100).unwrap();
    let second = runtime_config_version_after(first).unwrap();
    assert!(first > 100);
    assert!(second > first);
}

#[test]
fn reconnect_reason_projects_typed_scope_onto_the_rolling_update_wire() {
    assert_eq!(
        runtime_config_reload_reason(
            &scope(true, &[RuntimeConfigReconcileResource::PortForwarding],),
            "fallback",
        ),
        "agent_reconnect_authoritative_port_forwarding_sync"
    );
    assert_eq!(
        runtime_config_reload_reason(&scope(true, &[]), "fallback"),
        "agent_reconnect_authoritative_sync"
    );
    assert_eq!(
        runtime_config_reload_reason(
            &scope(false, &[RuntimeConfigReconcileResource::PortForwarding],),
            "fallback",
        ),
        "agent_reconnect_port_forwarding_sync"
    );
    assert_eq!(
        runtime_config_reload_reason(
            &scope(false, &[RuntimeConfigReconcileResource::RuntimeTunnels],),
            "fallback",
        ),
        "agent_reconnect_runtime_tunnels_sync"
    );
    assert_eq!(
        runtime_config_reload_reason(&scope(false, &[]), "fallback"),
        "fallback"
    );
}

#[test]
fn dispatch_errors_explain_impact_and_recovery_without_leaking_internal_details() {
    let internal = ApiError::from(anyhow::anyhow!("private database detail"));
    let internal_message = operator_dispatch_error(&internal, "Runtime apply job");
    assert!(internal_message.contains("Desired state remains saved"));
    assert!(internal_message.contains("inspect API logs and retry"));
    assert!(!internal_message.contains("private database detail"));

    let conflict = ApiError::conflict("agent_command_queue_full");
    let conflict_message = operator_dispatch_error(&conflict, "Runtime apply job");
    assert!(conflict_message.contains("agent command queue full"));
    assert!(conflict_message.contains("refresh target state"));

    let public = ApiError::bad_request_with_message(
        "runtime_config_invalid",
        "The rendered config is invalid for this VPS",
    );
    let public_message = operator_dispatch_error(&public, "Runtime apply job");
    assert!(public_message.contains("The rendered config is invalid for this VPS"));
    assert!(public_message.contains("Desired state remains saved"));
}

#[test]
fn operator_projections_remove_runtime_tunnel_credentials_recursively() {
    let mut value = serde_json::json!({
        "network": {
            "runtime_status_telemetry_plans": [{
                "plan_id": "plan",
                "builtin_credentials": {"private_key": "must-not-leak"},
                "nested": [{"builtin_credentials": {"certificate": "must-not-leak"}}]
            }]
        },
        "unrelated": "retained"
    });

    redact_runtime_tunnel_credentials(&mut value);

    let rendered = serde_json::to_string(&value).unwrap();
    assert!(!rendered.contains("builtin_credentials"));
    assert!(!rendered.contains("must-not-leak"));
    assert_eq!(value["unrelated"], "retained");
}

#[test]
fn typed_operator_config_projection_remains_toml_serializable() {
    let mut config = AgentConfig::default();
    config.noise.client_private_key_hex = None;
    config.telemetry.hostname_file = None;

    clear_runtime_tunnel_credentials(&mut config.network);

    let sections = serde_json::to_value(&config).unwrap();
    assert!(sections["noise"]["client_private_key_hex"].is_null());
    let rendered = toml::to_string_pretty(&config).unwrap();
    assert!(!rendered.contains("builtin_credentials"));
}
