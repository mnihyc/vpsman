use crate::vty_jobs::VtyProofContext;

const PRIVILEGE_HELP: &str =
    "Privilege commands: enable | disable | show privilege | show capabilities | show degraded-policy | proof";

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
    "pools",
    "tags",
    "jobs",
    "job-targets",
    "job-outputs",
    "job-follow",
    "terminal-sessions",
    "terminal-replay",
    "terminal-follow",
    "file-transfers",
    "file-transfer-sources",
    "process-supervisor-inventory",
    "super-password-rotations",
    "backups",
    "backup-artifacts",
    "backup-policies",
    "restore-plans",
    "migration-links",
    "tunnel-plans",
    "network-observations",
    "network-trends",
    "network-ospf-recommendations",
    "network-ospf-update-plans",
    "topology-graph",
    "audit",
    "history-export",
];

const PROOF_REQUIRED_COMMANDS: &[&str] = &[
    "job-create",
    "job-shell",
    "terminal-open",
    "terminal-input",
    "terminal-resize",
    "terminal-close",
    "file-pull",
    "file-push",
    "file-transfer-upload",
    "file-transfer-download",
    "user-sessions",
    "hot-config",
    "data-source-hot-config-apply",
    "agent-update",
    "agent-update-activate",
    "agent-update-rollback",
    "agent-update-rollout-activate",
    "agent-update-rollout-rollback",
    "agent-update-rollout-delegate-activation",
    "agent-update-rollout-delegate-rollback",
    "schedule-dispatch",
    "process-list",
    "process-start",
    "process-stop",
    "process-restart",
    "process-status",
    "process-logs",
    "backup-run",
    "restore-plan",
    "restore-run",
    "restore-rollback",
    "migration-run",
    "tunnel-apply",
    "tunnel-ospf-cost-update",
    "tunnel-rollback",
    "tunnel-status",
    "tunnel-probe",
    "tunnel-speed-test",
];

const FORCE_UNPRIVILEGED_COMMANDS: &[&str] = &[
    "schedule-dispatch",
    "process-start",
    "hot-config",
    "data-source-hot-config-apply",
    "agent-update",
    "agent-update-activate",
    "agent-update-rollback",
    "agent-update-rollout-activate",
    "agent-update-rollout-rollback",
    "agent-update-rollout-delegate-activation",
    "agent-update-rollout-delegate-rollback",
    "restore-run",
    "restore-rollback",
    "migration-run",
    "tunnel-apply",
    "tunnel-ospf-cost-update",
    "tunnel-rollback",
];

const ROOT_SENSITIVE_CAPABILITIES: &[&str] = &[
    "runtime tunnel reconciliation",
    "Bird2 managed-file reload",
    "agent binary activation and self-restart",
    "root-owned backup/restore paths",
    "process cgroup and rlimit enforcement",
    "privileged file writes",
];

pub(crate) fn vty_privilege_help() -> &'static str {
    PRIVILEGE_HELP
}

pub(crate) fn render_vty_privilege_status(proof_context: &VtyProofContext) -> String {
    pretty_json(serde_json::json!({
        "enabled": proof_context.enabled,
        "prompt": if proof_context.enabled { "vpsman#" } else { "vpsman>" },
        "proof_material": {
            "super_password_loaded_locally": proof_context.enabled && !proof_context.password.is_empty(),
            "salt_loaded_locally": proof_context.enabled && !proof_context.salt_hex.is_empty(),
            "plaintext_super_password_sent_to_server": false,
            "source": "VPSMAN_SUPER_PASSWORD and VPSMAN_SUPER_SALT_HEX environment variables",
            "redaction": "password and salt values are never printed by VTY status commands"
        },
        "next_steps": [
            "run enable before proof-gated mutations",
            "run disable to clear local proof material from this VTY session",
            "run show capabilities for proof and degraded-operation coverage"
        ]
    }))
}

pub(crate) fn render_vty_capabilities() -> String {
    pretty_json(serde_json::json!({
        "read_only_without_enable": READ_ONLY_COMMANDS,
        "proof_required_after_enable": PROOF_REQUIRED_COMMANDS,
        "force_unprivileged_supported": FORCE_UNPRIVILEGED_COMMANDS,
        "root_sensitive_capabilities": ROOT_SENSITIVE_CAPABILITIES,
        "proof_model": {
            "local_enable_command": "enable",
            "local_disable_command": "disable",
            "server_receives": "per-target proof envelopes and payload hashes",
            "server_never_receives": "plaintext super password"
        }
    }))
}

pub(crate) fn render_vty_degraded_policy() -> String {
    pretty_json(serde_json::json!({
        "default_result_when_agent_lacks_capability": "degraded_unprivileged",
        "force_flag": "--force-unprivileged",
        "policy": [
            "root-only mutations are reported as degraded by default on normal-user agents",
            "force-unprivileged is an explicit operator best-effort request where the command supports it",
            "unsupported capabilities should return typed degraded or unsupported status, not silent success",
            "observation commands remain useful on unprivileged agents and should include capability hints"
        ],
        "frequent_use_guidance": [
            "inspect show capabilities before bulk operations across mixed VPS environments",
            "prefer tags and pools to target agents with similar privilege and distro capabilities",
            "review job target status for ready, degraded, forced, or unsupported outcomes"
        ]
    }))
}

fn pretty_json(value: serde_json::Value) -> String {
    serde_json::to_string_pretty(&value).expect("static VTY privilege JSON renders")
}

#[cfg(test)]
mod tests {
    use super::{
        render_vty_capabilities, render_vty_degraded_policy, render_vty_privilege_status,
        vty_privilege_help,
    };
    use crate::vty_jobs::VtyProofContext;

    #[test]
    fn privilege_status_redacts_local_secret_material() {
        let proof_context = VtyProofContext {
            enabled: true,
            password: "do-not-print-this-password".to_string(),
            salt_hex: "0123456789abcdef0123456789abcdef".to_string(),
        };

        let rendered = render_vty_privilege_status(&proof_context);

        assert!(rendered.contains("\"enabled\": true"));
        assert!(rendered.contains("\"prompt\": \"vpsman#\""));
        assert!(rendered.contains("\"plaintext_super_password_sent_to_server\": false"));
        assert!(!rendered.contains(&proof_context.password));
        assert!(!rendered.contains(&proof_context.salt_hex));
    }

    #[test]
    fn capability_rendering_names_force_and_degraded_paths() {
        let capabilities = render_vty_capabilities();
        let degraded = render_vty_degraded_policy();

        assert!(capabilities.contains("force_unprivileged_supported"));
        assert!(capabilities.contains("tunnel-apply"));
        assert!(capabilities.contains("plaintext super password"));
        assert!(degraded.contains("degraded_unprivileged"));
        assert!(degraded.contains("--force-unprivileged"));
    }

    #[test]
    fn privilege_help_lists_router_style_affordances() {
        let help = vty_privilege_help();

        assert!(help.contains("enable"));
        assert!(help.contains("disable"));
        assert!(help.contains("show privilege"));
        assert!(help.contains("show capabilities"));
        assert!(help.contains("show degraded-policy"));
    }
}
