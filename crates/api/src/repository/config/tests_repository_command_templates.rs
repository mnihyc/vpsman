use super::*;

fn shell_template_request(name: &str, scope_kind: &str) -> UpsertCommandTemplateRequest {
    UpsertCommandTemplateRequest {
        name: name.to_string(),
        scope_kind: scope_kind.to_string(),
        scope_value: None,
        display_group: None,
        operation: serde_json::json!({
            "type": "shell",
            "argv": ["/usr/bin/uptime"],
            "pty": false
        }),
        defaults: serde_json::json!({
            "max_timeout_secs": 30,
            "confirmed": false
        }),
        confirmed: true,
    }
}

#[test]
fn runtime_config_sync_command_templates_are_forbidden() {
    let mut request = shell_template_request("forbidden runtime sync", "global");
    request.operation = serde_json::to_value(JobCommand::RuntimeConfigSync {
        desired_version: 1,
        reason: "template must not own desired state".to_string(),
        config: Box::new(vpsman_common::AgentRuntimeConfig::default()),
    })
    .unwrap();
    let error = validate_command_template_request(&request).unwrap_err();
    assert!(error.to_string().contains("server-issued"));
}
