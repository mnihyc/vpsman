use anyhow::{Context, Result};
use argon2::{Algorithm, Argon2, Params, Version};
use axum::http::HeaderMap;
use rand::RngCore;
use vpsman_common::payload_hash;
pub(crate) use vpsman_server_core::{
    default_operator_scopes, operator_has_scope, operator_role_rank, role_allows,
};

use crate::error::ApiError;

pub(crate) const ACCESS_TOKEN_TTL_SECS: u64 = 24 * 60 * 60;
pub(crate) const DEFAULT_REFRESH_TOKEN_TTL_SECS: u64 = 365 * 24 * 60 * 60;
pub(crate) const MIN_REFRESH_TOKEN_TTL_SECS: u64 = 24 * 60 * 60;
pub(crate) const MAX_REFRESH_TOKEN_TTL_SECS: u64 = 10 * 365 * 24 * 60 * 60;
pub(crate) const SCOPE_FLEET_READ: &str = "fleet:read";
pub(crate) const SCOPE_JOBS_READ: &str = "jobs:read";
pub(crate) const SCOPE_BACKUPS_READ: &str = "backups:read";
pub(crate) const SCOPE_TERMINAL_READ: &str = "terminal:read";
pub(crate) const SCOPE_INTEGRATIONS_READ: &str = "integrations:read";
pub(crate) const SCOPE_TEMPLATES_READ: &str = "templates:read";
pub(crate) const SCOPE_SCHEDULES_READ: &str = "schedules:read";
pub(crate) const SCOPE_CONFIG_READ: &str = "config:read";
pub(crate) const SCOPE_NETWORK_READ: &str = "network:read";
const ARGON2_MEMORY_KIB: u32 = 19_456;
const ARGON2_TIME_COST: u32 = 2;
const ARGON2_PARALLELISM: u32 = 1;

pub(crate) fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    let value = headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?;
    value.strip_prefix("Bearer ")
}

pub(crate) fn validate_operator_credentials(
    username: &str,
    password: &str,
) -> Result<(), ApiError> {
    if username.trim().is_empty() {
        return Err(ApiError::bad_request("username_required"));
    }
    if password.len() < 12 {
        return Err(ApiError::bad_request("password_too_short"));
    }
    Ok(())
}

pub(crate) fn validate_operator_role(role: &str) -> Result<(), ApiError> {
    if operator_role_rank(role.trim()).is_some() {
        Ok(())
    } else {
        Err(ApiError::bad_request("invalid_operator_role"))
    }
}

pub(crate) fn normalize_operator_scopes(
    role: &str,
    scopes: &[String],
) -> Result<Vec<String>, ApiError> {
    let mut scopes = scopes
        .iter()
        .map(|scope| scope.trim())
        .filter(|scope| !scope.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    if scopes.is_empty() {
        scopes = default_operator_scopes(role);
    }
    if scopes.len() > 32 {
        return Err(ApiError::bad_request("too_many_operator_scopes"));
    }
    for scope in &scopes {
        validate_operator_scope(role, scope)?;
    }
    scopes.sort();
    scopes.dedup();
    Ok(scopes)
}

fn validate_operator_scope(role: &str, scope: &str) -> Result<(), ApiError> {
    if scope.len() > 64
        || !scope.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'-' | b'_' | b'.' | b'*')
        })
    {
        return Err(ApiError::bad_request("invalid_operator_scope"));
    }
    if scope == "*" && role.trim() != "admin" {
        return Err(ApiError::bad_request("wildcard_scope_requires_admin"));
    }
    Ok(())
}

pub(crate) fn hash_operator_password(password: &str) -> Result<String> {
    let mut salt = [0_u8; 16];
    rand::thread_rng().fill_bytes(&mut salt);
    let digest = argon2_digest(password.as_bytes(), &salt)?;
    Ok(format!(
        "argon2id$v=19$m={ARGON2_MEMORY_KIB},t={ARGON2_TIME_COST},p={ARGON2_PARALLELISM}${}${}",
        hex::encode(salt),
        hex::encode(digest)
    ))
}

pub(crate) fn verify_operator_password(password: &str, encoded: &str) -> Result<bool> {
    let parts = encoded.split('$').collect::<Vec<_>>();
    if parts.len() != 5 || parts[0] != "argon2id" || parts[1] != "v=19" {
        return Ok(false);
    }
    let salt = hex::decode(parts[3]).context("invalid operator password salt")?;
    let expected = hex::decode(parts[4]).context("invalid operator password hash")?;
    let digest = argon2_digest(password.as_bytes(), &salt)?;
    Ok(constant_time_eq(&digest, &expected))
}

pub(crate) fn argon2_digest(password: &[u8], salt: &[u8]) -> Result<[u8; 32]> {
    let params = Params::new(
        ARGON2_MEMORY_KIB,
        ARGON2_TIME_COST,
        ARGON2_PARALLELISM,
        Some(32),
    )
    .map_err(|error| anyhow::anyhow!("invalid Argon2 params: {error}"))?;
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut output = [0_u8; 32];
    argon2
        .hash_password_into(password, salt, &mut output)
        .map_err(|error| anyhow::anyhow!("failed to hash operator password: {error}"))?;
    Ok(output)
}

pub(crate) fn generate_token() -> String {
    let mut token = [0_u8; 32];
    rand::thread_rng().fill_bytes(&mut token);
    hex::encode(token)
}

pub(crate) fn token_hash(token: &str) -> String {
    payload_hash(token.as_bytes())
}

pub(crate) fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter()
        .zip(b.iter())
        .fold(0_u8, |acc, (left, right)| acc | (left ^ right))
        == 0
}
