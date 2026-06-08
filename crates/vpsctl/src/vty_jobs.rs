use anyhow::{Context, Result};
use serde::Deserialize;
use vpsman_common::JobCommand;

use crate::{
    commands_schedules::selector_expression_from_targets,
    http::http_post_json,
    privilege::{build_privilege_for_job_command, load_super_password, load_super_salt_hex},
};

#[derive(Clone, Debug, Default)]
pub(crate) struct VtyPrivilegeContext {
    pub(crate) enabled: bool,
    pub(crate) password: String,
    pub(crate) salt_hex: String,
}

#[derive(Debug, Deserialize)]
struct VtyBulkResolveResponse {
    targets: Vec<VtyTarget>,
}

#[derive(Debug, Deserialize)]
struct VtyTarget {
    id: String,
}

#[derive(Debug, Default, Eq, PartialEq)]
pub(crate) struct VtyJobSelection {
    pub(crate) clients: Vec<String>,
    pub(crate) tags: Vec<String>,
    pub(crate) destructive: bool,
    pub(crate) confirmed: bool,
}

impl VtyPrivilegeContext {
    pub(crate) fn from_env() -> Result<Self> {
        let password = load_super_password("VPSMAN_SUPER_PASSWORD")?;
        let salt_hex = load_super_salt_hex(None)?;
        Ok(Self {
            enabled: true,
            password,
            salt_hex,
        })
    }
}

impl VtyJobSelection {
    pub(crate) fn parse(tokens: &[&str]) -> Result<Self> {
        let mut selection = Self::default();
        for token in tokens {
            match *token {
                "--destructive" => selection.destructive = true,
                "--confirmed" => selection.confirmed = true,
                "" => {}
                value => {
                    if let Some(target) = value.strip_prefix("tag:") {
                        anyhow::ensure!(!target.is_empty(), "empty target in {value}");
                        selection.tags.push(target.to_string());
                    } else {
                        selection.tags.push(value.to_string());
                    }
                }
            }
        }
        selection.clients.sort();
        selection.clients.dedup();
        selection.tags.sort();
        selection.tags.dedup();
        anyhow::ensure!(
            !selection.clients.is_empty() || !selection.tags.is_empty(),
            "job target selection requires at least one client or tag target"
        );
        Ok(selection)
    }
}

pub(crate) fn vty_create_job(
    api_url: &str,
    token: Option<&str>,
    privilege_context: &VtyPrivilegeContext,
    command: &str,
    pty: bool,
    selection: VtyJobSelection,
) -> Result<String> {
    let operation = JobCommand::Shell {
        argv: vec![command.to_string()],
        pty,
    };
    vty_submit_operation(
        api_url,
        token,
        privilege_context,
        if pty { "shell_pty" } else { command },
        &operation,
        selection,
        30,
    )
}

pub(crate) fn vty_create_shell_script(
    api_url: &str,
    token: Option<&str>,
    privilege_context: &VtyPrivilegeContext,
    script: &str,
    selection: VtyJobSelection,
) -> Result<String> {
    anyhow::ensure!(!script.trim().is_empty(), "job-shell script is empty");
    let operation = JobCommand::ShellScript {
        script: script.to_string(),
    };
    vty_submit_operation(
        api_url,
        token,
        privilege_context,
        "shell_script",
        &operation,
        selection,
        30,
    )
}

pub(crate) fn vty_submit_operation(
    api_url: &str,
    token: Option<&str>,
    privilege_context: &VtyPrivilegeContext,
    command_label: &str,
    operation: &JobCommand,
    selection: VtyJobSelection,
    timeout_secs: u64,
) -> Result<String> {
    vty_submit_operation_with_force(
        api_url,
        token,
        privilege_context,
        command_label,
        operation,
        selection,
        timeout_secs,
        false,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn vty_submit_operation_with_force(
    api_url: &str,
    token: Option<&str>,
    privilege_context: &VtyPrivilegeContext,
    command_label: &str,
    operation: &JobCommand,
    selection: VtyJobSelection,
    timeout_secs: u64,
    force_unprivileged: bool,
) -> Result<String> {
    let resolved = http_post_json(
        api_url,
        "/api/v1/bulk/resolve",
        token,
        &serde_json::json!({
            "selector_expression": selector_expression_from_targets(&selection.clients, &selection.tags),
        }),
    )?;
    let resolved: VtyBulkResolveResponse =
        serde_json::from_str(&resolved).context("failed to parse bulk target response")?;
    let client_ids = resolved
        .targets
        .into_iter()
        .map(|target| target.id)
        .collect::<Vec<_>>();
    anyhow::ensure!(
        !client_ids.is_empty(),
        "{command_label} resolved no targets; provide at least one matching target"
    );
    let selector_expression = selector_expression_from_targets(&selection.clients, &selection.tags);
    let privilege = build_privilege_for_job_command(
        &client_ids,
        operation,
        command_label,
        &selector_expression,
        &privilege_context.password,
        &privilege_context.salt_hex,
        300,
        timeout_secs,
        None,
        force_unprivileged,
        true,
    )?;

    http_post_json(
        api_url,
        "/api/v1/jobs",
        token,
        &serde_json::json!({
            "command": command_label,
            "argv": [],
            "operation": operation,
            "selector_expression": selector_expression,
            "privileged": true,
            "destructive": selection.destructive,
            "confirmed": selection.confirmed,
            "force_unprivileged": force_unprivileged,
            "timeout_secs": timeout_secs,
            "privilege_assertion": privilege.privilege_assertion,
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::VtyJobSelection;

    #[test]
    fn parses_explicit_vty_job_targets_and_flags() {
        let selection = VtyJobSelection::parse(&[
            "id:client-a",
            "name:edge-a",
            "pool:edge",
            "provider:alpha",
            "country:US",
            "tag:bgp",
            "edge",
            "--destructive",
            "--confirmed",
            "id:client-a",
        ])
        .unwrap();

        assert!(selection.clients.is_empty());
        assert_eq!(
            selection.tags,
            vec![
                "bgp",
                "country:US",
                "edge",
                "id:client-a",
                "name:edge-a",
                "pool:edge",
                "provider:alpha"
            ]
        );
        assert!(selection.destructive);
        assert!(selection.confirmed);
    }

    #[test]
    fn treats_namespaced_values_as_tags_and_rejects_empty_selectors() {
        let selection = VtyJobSelection::parse(&["client:edge-a", "role:edge"]).unwrap();
        assert_eq!(selection.tags, vec!["client:edge-a", "role:edge"]);
        assert!(VtyJobSelection::parse(&["tag:"]).is_err());
        assert!(VtyJobSelection::parse(&["--destructive"]).is_err());
    }
}
