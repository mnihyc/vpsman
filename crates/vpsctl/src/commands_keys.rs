use anyhow::Result;
use vpsman_common::generate_noise_keypair;

use crate::http::{http_get, http_post_json};
use crate::privilege::{
    build_privilege_for_db, load_super_password, load_super_salt_hex, DbPrivilegeRequest,
};

pub(crate) fn noise_keygen() -> Result<()> {
    let keypair = generate_noise_keypair()?;
    println!(
        "{}",
        serde_json::json!({
            "private_key_hex": keypair.private_hex(),
            "public_key_hex": keypair.public_hex()
        })
    );
    Ok(())
}

pub(crate) struct AgentIdentityUpsertOptions {
    pub(crate) client_id: String,
    pub(crate) client_public_key_hex: String,
    pub(crate) display_name: Option<String>,
    pub(crate) tags: Vec<String>,
    pub(crate) replace_existing_key: bool,
    pub(crate) confirmed: bool,
}

pub(crate) fn agent_identity_upsert(
    api_url: &str,
    token: Option<&str>,
    options: AgentIdentityUpsertOptions,
) -> Result<()> {
    let password = load_super_password("VPSMAN_SUPER_PASSWORD")?;
    let salt_hex = load_super_salt_hex(None)?;
    let targets = vec![options.client_id.clone()];
    let privilege_assertion = build_privilege_for_db(
        DbPrivilegeRequest {
            action: if options.replace_existing_key {
                "agent_identity.rotate"
            } else {
                "agent_identity.import"
            },
            target: &options.client_id,
            selector_expression: None,
            resolved_targets: &targets,
            confirmed: options.confirmed,
        },
        &password,
        &salt_hex,
        300,
    )?;
    let body = serde_json::json!({
        "client_id": options.client_id,
        "client_public_key_hex": options.client_public_key_hex,
        "display_name": options.display_name,
        "tags": options.tags,
        "replace_existing_key": options.replace_existing_key,
        "confirmed": options.confirmed,
        "privilege_assertion": privilege_assertion,
    });
    println!(
        "{}",
        http_post_json(api_url, "/api/v1/agent-identities", token, &body,)?
    );
    Ok(())
}

pub(crate) fn client_key_revocations(api_url: &str, token: Option<&str>, limit: u16) -> Result<()> {
    println!(
        "{}",
        http_get(
            api_url,
            &format!("/api/v1/client-key-revocations?limit={}", limit),
            token,
        )?
    );
    Ok(())
}

pub(crate) fn client_key_revoke(
    api_url: &str,
    token: Option<&str>,
    client_id: String,
    reason: Option<String>,
    confirmed: bool,
) -> Result<()> {
    let password = load_super_password("VPSMAN_SUPER_PASSWORD")?;
    let salt_hex = load_super_salt_hex(None)?;
    let targets = vec![client_id.clone()];
    let privilege_assertion = build_privilege_for_db(
        DbPrivilegeRequest {
            action: "client_key.revoke",
            target: &client_id,
            selector_expression: None,
            resolved_targets: &targets,
            confirmed,
        },
        &password,
        &salt_hex,
        300,
    )?;
    println!(
        "{}",
        http_post_json(
            api_url,
            &format!("/api/v1/clients/{client_id}/key-revocations"),
            token,
            &serde_json::json!({
                "reason": reason,
                "confirmed": confirmed,
                "privilege_assertion": privilege_assertion,
            }),
        )?
    );
    Ok(())
}

pub(crate) fn key_lifecycle_report(api_url: &str, token: Option<&str>) -> Result<()> {
    println!(
        "{}",
        http_get(api_url, "/api/v1/key-lifecycle/report", token)?
    );
    Ok(())
}
