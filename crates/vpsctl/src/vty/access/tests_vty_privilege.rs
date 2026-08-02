use super::{
    render_vty_capabilities, render_vty_degraded_policy, render_vty_privilege_status,
    vty_privilege_help,
};
use crate::vty_jobs::VtyPrivilegeContext;

#[test]
fn privilege_status_redacts_local_secret_material() {
    let privilege_context = VtyPrivilegeContext {
        enabled: true,
        password: "do-not-print-this-password".to_string(),
        salt_hex: "0123456789abcdef0123456789abcdef".to_string(),
    };

    let rendered = render_vty_privilege_status(&privilege_context);

    assert!(rendered.contains("\"enabled\": true"));
    assert!(rendered.contains("\"prompt\": \"vpsman#\""));
    assert!(rendered.contains("\"plaintext_super_password_sent_to_server\": false"));
    assert!(!rendered.contains(&privilege_context.password));
    assert!(!rendered.contains(&privilege_context.salt_hex));
}

#[test]
fn capability_rendering_names_force_and_degraded_paths() {
    let capabilities = render_vty_capabilities();
    let degraded = render_vty_degraded_policy();

    assert!(capabilities.contains("force_unprivileged_supported"));
    assert!(capabilities.contains("tunnel-speed-test"));
    assert!(capabilities.contains("plaintext super password"));
    assert!(degraded.contains("target_skipped_job_partial_success"));
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
