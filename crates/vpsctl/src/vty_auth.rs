use anyhow::{bail, Context, Result};

use crate::http::http_post_json;

pub(crate) fn is_vty_totp_command(command: &str) -> bool {
    matches!(command, "totp-setup" | "totp-confirm" | "totp-disable")
        || command.starts_with("totp-setup ")
        || command.starts_with("totp-confirm ")
        || command.starts_with("totp-disable ")
}

pub(crate) fn submit_vty_totp_command(
    api_url: &str,
    token: Option<&str>,
    command: &str,
) -> Result<String> {
    let parts = command.split_whitespace().collect::<Vec<_>>();
    let Some(name) = parts.first().copied() else {
        bail!("empty TOTP command");
    };
    match name {
        "totp-setup" => {
            if parts.len() > 2 {
                bail!("usage: totp-setup [password_env]");
            }
            let password = read_env(parts.get(1).copied().unwrap_or("VPSMAN_OPERATOR_PASSWORD"))?;
            http_post_json(
                api_url,
                "/api/v1/auth/totp/setup",
                token,
                &serde_json::json!({ "password": password }),
            )
        }
        "totp-confirm" | "totp-disable" => {
            if parts.len() > 3 {
                bail!("usage: {name} [password_env] [code_env]");
            }
            let password = read_env(parts.get(1).copied().unwrap_or("VPSMAN_OPERATOR_PASSWORD"))?;
            let code = read_env(parts.get(2).copied().unwrap_or("VPSMAN_TOTP_CODE"))?;
            let path = if name == "totp-confirm" {
                "/api/v1/auth/totp/confirm"
            } else {
                "/api/v1/auth/totp/disable"
            };
            http_post_json(
                api_url,
                path,
                token,
                &serde_json::json!({
                    "password": password,
                    "code": code,
                }),
            )
        }
        _ => bail!("unknown TOTP command"),
    }
}

fn read_env(name: &str) -> Result<String> {
    std::env::var(name).with_context(|| format!("environment variable {name} is not set"))
}
