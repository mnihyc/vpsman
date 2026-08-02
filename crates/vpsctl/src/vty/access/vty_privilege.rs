use crate::vty_jobs::VtyPrivilegeContext;

const PRIVILEGE_HELP: &str =
    "Privilege commands: enable | disable | show privilege | show capabilities | show degraded-policy";

const READ_ONLY_COMMANDS: &[&str] = &[
    "health",
    "summary",
    "agents",
    "fleet-alerts",
    "fleet-alert-notifications",
    "gateway-sessions",
    "telemetry-rollups",
    "telemetry-network-rates",
    "telemetry-tunnels",
    "tags",
    "jobs",
    "job-targets",
    "job-target-status-download",
    "job-outputs",
    "job-follow",
    "job-output-download",
    "terminal-sessions",
    "terminal-replay",
    "terminal-follow",
    "terminal-poll",
    "file-transfers",
    "file-transfer-sources",
    "process-supervisor-inventory",
    "host-process-refresh",
    "host-processes",
    "backups",
    "backup-artifacts",
    "backup-policies",
    "restore-plans",
    "migration-links",
    "tunnel-plans",
    "port-forwards",
    "port-forward-resolve",
    "tunnel-plan-export",
    "network-observations",
    "network-trends",
    "network-ospf-recommendations",
    "network-ospf-update-plans",
    "topology-graph",
    "audit",
    "history-export",
];

const PRIVILEGE_REQUIRED_COMMANDS: &[&str] = &[
    "job-create",
    "job-shell",
    "terminal-open",
    "file-pull",
    "file-push",
    "file-transfer-upload",
    "file-transfer-download",
    "user-sessions",
    "agent-update",
    "agent-update-activate",
    "agent-update-rollback",
    "process-start",
    "process-stop",
    "process-restart",
    "process-status",
    "process-logs",
    "backup-run",
    "restore-plan",
    "restore-run",
    "restore-rollback",
    "migration-link",
    "migration-run",
    "tunnel-probe",
    "tunnel-speed-test",
];

const SESSION_AUTHORIZED_COMMANDS: &[&str] =
    &["terminal-input", "terminal-resize", "terminal-close"];

const FORCE_UNPRIVILEGED_COMMANDS: &[&str] = &[
    "process-start",
    "agent-update",
    "agent-update-activate",
    "agent-update-rollback",
    "restore-run",
    "restore-rollback",
    "migration-run",
];

const ROOT_SENSITIVE_CAPABILITIES: &[&str] = &[
    "runtime tunnel reconciliation",
    "operator-bound routing adapter execution",
    "agent binary activation and self-restart",
    "root-owned backup/restore paths",
    "process cgroup and rlimit enforcement",
    "privileged file writes",
];

pub(crate) fn vty_privilege_help() -> &'static str {
    PRIVILEGE_HELP
}

pub(crate) fn render_vty_privilege_status(privilege_context: &VtyPrivilegeContext) -> String {
    pretty_json(serde_json::json!({
        "enabled": privilege_context.enabled,
        "prompt": if privilege_context.enabled { "vpsman#" } else { "vpsman>" },
        "privilege_material": {
            "super_password_loaded_locally": privilege_context.enabled && !privilege_context.password.is_empty(),
            "salt_loaded_locally": privilege_context.enabled && !privilege_context.salt_hex.is_empty(),
            "plaintext_super_password_sent_to_server": false,
            "source": "VPSMAN_SUPER_PASSWORD and VPSMAN_SUPER_SALT_HEX environment variables",
            "redaction": "password and salt values are never printed by VTY status commands"
        },
        "next_steps": [
            "run enable before privilege-gated mutations",
            "run disable to clear local privilege unlock material from this VTY session",
            "run show capabilities for privilege unlock and degraded-operation coverage"
        ]
    }))
}

pub(crate) fn render_vty_capabilities() -> String {
    pretty_json(serde_json::json!({
        "read_only_without_enable": READ_ONLY_COMMANDS,
        "privilege_required_after_enable": PRIVILEGE_REQUIRED_COMMANDS,
        "authorized_by_open_terminal_session": SESSION_AUTHORIZED_COMMANDS,
        "force_unprivileged_supported": FORCE_UNPRIVILEGED_COMMANDS,
        "root_sensitive_capabilities": ROOT_SENSITIVE_CAPABILITIES,
        "privilege_model": {
            "local_enable_command": "enable",
            "local_disable_command": "disable",
            "server_receives": "request-bound privilege assertions and payload hashes",
            "server_never_receives": "plaintext super password"
        }
    }))
}

pub(crate) fn render_vty_degraded_policy() -> String {
    pretty_json(serde_json::json!({
        "default_result_when_agent_lacks_capability": "target_skipped_job_partial_success",
        "force_flag": "--force-unprivileged",
        "policy": [
            "root-only mutations are skipped by default on normal-user agents",
            "force-unprivileged is an explicit operator best-effort request where the command supports it",
            "unsupported capabilities should return typed skipped or unsupported status, not silent success",
            "observation commands remain useful on unprivileged agents and should include capability hints"
        ],
        "frequent_use_guidance": [
            "inspect show capabilities before bulk operations across mixed VPS environments",
            "prefer tags to target agents with similar privilege and distro capabilities",
            "review job target status for ready, degraded, forced, or unsupported outcomes"
        ]
    }))
}

fn pretty_json(value: serde_json::Value) -> String {
    serde_json::to_string_pretty(&value).expect("static VTY privilege JSON renders")
}

#[cfg(test)]
#[path = "tests_vty_privilege.rs"]
mod tests;
