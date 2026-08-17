use super::*;
use crate::{
    model::{OperatorPreferences, OperatorView},
    repository::{MemoryState, Repository},
    DEFAULT_REFRESH_TOKEN_TTL_SECS,
};

fn test_operator() -> AuthContext {
    AuthContext {
        operator: OperatorView {
            id: Uuid::new_v4(),
            username: "template-admin".to_string(),
            role: "admin".to_string(),
            scopes: vec!["*".to_string()],
            preferences: OperatorPreferences::default(),
            totp_enabled: false,
            status: "active".to_string(),
            session_refresh_ttl_secs: DEFAULT_REFRESH_TOKEN_TTL_SECS,
            created_at: unix_now().to_string(),
            disabled_at: None,
            deleted_at: None,
        },
        session_id: Some(Uuid::new_v4()),
    }
}

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

#[tokio::test]
async fn command_template_builtins_are_listed_and_immutable() {
    let repo = Repository::Memory(MemoryState::default());
    let templates = repo
        .list_command_templates(20, None, None, None, None)
        .await
        .unwrap();

    let shell = templates
        .iter()
        .find(|template| template.name == "Default shell command")
        .expect("default shell builtin missing");
    assert!(shell.built_in);
    assert_eq!(shell.scope_kind, "global");
    assert_eq!(shell.command_type, "shell_argv");

    let updates = repo
        .list_command_templates(20, None, None, Some("agent_update_check"), None)
        .await
        .unwrap();
    assert_eq!(updates.len(), 1);
    assert!(updates[0].built_in);
    assert_eq!(updates[0].name, "Default manual update check");

    let error = repo
        .upsert_command_template(
            &shell_template_request("Default shell command", "global"),
            &test_operator(),
        )
        .await
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("command_template_builtin_immutable"));
}

#[tokio::test]
async fn command_template_delete_rejects_a_stale_reviewed_name() {
    let repo = Repository::Memory(MemoryState::default());
    let operator = test_operator();
    let created = repo
        .upsert_command_template(
            &shell_template_request("operator-health-check", "global"),
            &operator,
        )
        .await
        .unwrap();
    assert!(!created.built_in);

    let Repository::Memory(memory) = &repo else {
        unreachable!("test uses memory repository");
    };
    memory
        .command_templates
        .write()
        .await
        .iter_mut()
        .find(|template| template.id == created.id)
        .unwrap()
        .name = "operator-health-check-renamed".to_string();

    let error = repo
        .delete_command_template(created.id, &created.name, &operator)
        .await
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("command_template_delete_review_stale"));
    assert!(memory
        .command_templates
        .read()
        .await
        .iter()
        .any(|template| template.id == created.id));

    let deleted = repo
        .delete_command_template(created.id, "operator-health-check-renamed", &operator)
        .await
        .unwrap()
        .expect("user template should delete");
    assert_eq!(deleted.id, created.id);
    assert_eq!(deleted.name, "operator-health-check-renamed");
    assert!(!deleted.built_in);

    let templates = repo
        .list_command_templates(20, None, None, None, None)
        .await
        .unwrap();
    assert!(!templates.iter().any(|template| template.id == created.id));

    let audits = memory.audits.read().await;
    assert!(audits
        .iter()
        .any(|audit| audit.action == "command_template.upserted"));
    assert!(audits
        .iter()
        .any(|audit| audit.action == "command_template.deleted"));
}
