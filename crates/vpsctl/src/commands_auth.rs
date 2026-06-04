use anyhow::{Context, Result};

use crate::http::{http_delete, http_get, http_post_json};

pub(crate) fn health(api_url: &str) -> Result<()> {
    println!("{}", http_get(api_url, "/health", None)?);
    Ok(())
}

pub(crate) fn bootstrap(api_url: &str, username: String, password_env: String) -> Result<()> {
    let password = std::env::var(&password_env)
        .with_context(|| format!("environment variable {password_env} is not set"))?;
    println!(
        "{}",
        http_post_json(
            api_url,
            "/api/v1/auth/bootstrap",
            None,
            &serde_json::json!({
                "username": username,
                "password": password,
            }),
        )?
    );
    Ok(())
}

pub(crate) fn login(
    api_url: &str,
    username: String,
    password_env: String,
    totp_code: Option<String>,
) -> Result<()> {
    let password = std::env::var(&password_env)
        .with_context(|| format!("environment variable {password_env} is not set"))?;
    let mut body = serde_json::json!({
        "username": username,
        "password": password,
    });
    if let Some(totp_code) = totp_code {
        body["totp_code"] = serde_json::Value::String(totp_code);
    }
    println!(
        "{}",
        http_post_json(api_url, "/api/v1/auth/login", None, &body)?
    );
    Ok(())
}

pub(crate) fn refresh(api_url: &str, refresh_token_env: String) -> Result<()> {
    let refresh_token = std::env::var(&refresh_token_env)
        .with_context(|| format!("environment variable {refresh_token_env} is not set"))?;
    println!(
        "{}",
        http_post_json(
            api_url,
            "/api/v1/auth/refresh",
            None,
            &serde_json::json!({
                "refresh_token": refresh_token,
            }),
        )?
    );
    Ok(())
}

pub(crate) fn me(api_url: &str, token: Option<&str>) -> Result<()> {
    println!("{}", http_get(api_url, "/api/v1/auth/me", token)?);
    Ok(())
}

pub(crate) fn operators(api_url: &str, token: Option<&str>) -> Result<()> {
    println!("{}", http_get(api_url, "/api/v1/operators", token)?);
    Ok(())
}

pub(crate) fn operator_create(
    api_url: &str,
    token: Option<&str>,
    username: String,
    role: String,
    scopes: Vec<String>,
    password_env: String,
) -> Result<()> {
    let password = std::env::var(&password_env)
        .with_context(|| format!("environment variable {password_env} is not set"))?;
    println!(
        "{}",
        http_post_json(
            api_url,
            "/api/v1/operators",
            token,
            &serde_json::json!({
                "username": username,
                "password": password,
                "role": role,
                "scopes": scopes,
            }),
        )?
    );
    Ok(())
}

pub(crate) fn operator_sessions(api_url: &str, token: Option<&str>, limit: u16) -> Result<()> {
    println!(
        "{}",
        http_get(
            api_url,
            &format!("/api/v1/operator-sessions?limit={}", limit.clamp(1, 200)),
            token,
        )?
    );
    Ok(())
}

pub(crate) fn super_password_rotations(
    api_url: &str,
    token: Option<&str>,
    limit: u16,
) -> Result<()> {
    println!(
        "{}",
        http_get(
            api_url,
            &format!("/api/v1/auth/proof-rotations?limit={}", limit.clamp(1, 200)),
            token,
        )?
    );
    Ok(())
}

pub(crate) fn operator_session_revoke(
    api_url: &str,
    token: Option<&str>,
    session_id: String,
) -> Result<()> {
    println!(
        "{}",
        http_delete(
            api_url,
            &format!("/api/v1/operator-sessions/{session_id}"),
            token,
        )?
    );
    Ok(())
}

pub(crate) fn totp_setup(api_url: &str, token: Option<&str>, password_env: String) -> Result<()> {
    let password = std::env::var(&password_env)
        .with_context(|| format!("environment variable {password_env} is not set"))?;
    println!(
        "{}",
        http_post_json(
            api_url,
            "/api/v1/auth/totp/setup",
            token,
            &serde_json::json!({ "password": password }),
        )?
    );
    Ok(())
}

pub(crate) fn totp_confirm(
    api_url: &str,
    token: Option<&str>,
    password_env: String,
    code_env: String,
) -> Result<()> {
    submit_totp_update(
        api_url,
        token,
        "/api/v1/auth/totp/confirm",
        password_env,
        code_env,
    )
}

pub(crate) fn totp_disable(
    api_url: &str,
    token: Option<&str>,
    password_env: String,
    code_env: String,
) -> Result<()> {
    submit_totp_update(
        api_url,
        token,
        "/api/v1/auth/totp/disable",
        password_env,
        code_env,
    )
}

fn submit_totp_update(
    api_url: &str,
    token: Option<&str>,
    path: &str,
    password_env: String,
    code_env: String,
) -> Result<()> {
    let password = std::env::var(&password_env)
        .with_context(|| format!("environment variable {password_env} is not set"))?;
    let code = std::env::var(&code_env)
        .with_context(|| format!("environment variable {code_env} is not set"))?;
    println!(
        "{}",
        http_post_json(
            api_url,
            path,
            token,
            &serde_json::json!({
                "password": password,
                "code": code,
            }),
        )?
    );
    Ok(())
}
