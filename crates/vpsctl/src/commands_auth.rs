use anyhow::{Context, Result};

use crate::{
    http::{http_delete, http_get, http_post_json, http_put_json},
    util::percent_encode_query_value,
};

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
    session_refresh_ttl_secs: u64,
    admin_risk_acknowledged: bool,
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
                "session_refresh_ttl_secs": session_refresh_ttl_secs,
                "admin_risk_acknowledged": admin_risk_acknowledged,
            }),
        )?
    );
    Ok(())
}

pub(crate) fn operator_update(
    api_url: &str,
    token: Option<&str>,
    operator_id: String,
    role: String,
    scopes: Vec<String>,
    session_refresh_ttl_secs: u64,
    admin_risk_acknowledged: bool,
) -> Result<()> {
    println!(
        "{}",
        http_put_json(
            api_url,
            &format!("/api/v1/operators/{operator_id}"),
            token,
            &serde_json::json!({
                "role": role,
                "scopes": scopes,
                "session_refresh_ttl_secs": session_refresh_ttl_secs,
                "confirmed": true,
                "admin_risk_acknowledged": admin_risk_acknowledged,
            }),
        )?
    );
    Ok(())
}

pub(crate) fn operator_set_status(
    api_url: &str,
    token: Option<&str>,
    operator_id: String,
    action: &str,
    admin_risk_acknowledged: bool,
) -> Result<()> {
    println!(
        "{}",
        http_post_json(
            api_url,
            &format!("/api/v1/operators/{operator_id}/{action}"),
            token,
            &serde_json::json!({
                "confirmed": true,
                "admin_risk_acknowledged": admin_risk_acknowledged,
            }),
        )?
    );
    Ok(())
}

pub(crate) fn operator_password_reset(
    api_url: &str,
    token: Option<&str>,
    operator_id: String,
    password_env: String,
    admin_risk_acknowledged: bool,
) -> Result<()> {
    let password = std::env::var(&password_env)
        .with_context(|| format!("environment variable {password_env} is not set"))?;
    println!(
        "{}",
        http_post_json(
            api_url,
            &format!("/api/v1/operators/{operator_id}/password-reset"),
            token,
            &serde_json::json!({
                "password": password,
                "confirmed": true,
                "admin_risk_acknowledged": admin_risk_acknowledged,
            }),
        )?
    );
    Ok(())
}

pub(crate) fn operator_auth_events(
    api_url: &str,
    token: Option<&str>,
    limit: u16,
    operator_id: Option<String>,
    username: Option<String>,
    result: Option<String>,
) -> Result<()> {
    let mut params = vec![format!("limit={}", limit.clamp(1, 200))];
    if let Some(operator_id) = operator_id.filter(|value| !value.trim().is_empty()) {
        params.push(format!(
            "operator_id={}",
            percent_encode_query_value(&operator_id)
        ));
    }
    if let Some(username) = username.filter(|value| !value.trim().is_empty()) {
        params.push(format!(
            "username={}",
            percent_encode_query_value(&username)
        ));
    }
    if let Some(result) = result.filter(|value| !value.trim().is_empty()) {
        params.push(format!("result={}", percent_encode_query_value(&result)));
    }
    println!(
        "{}",
        http_get(
            api_url,
            &format!("/api/v1/operator-auth-events?{}", params.join("&")),
            token,
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
