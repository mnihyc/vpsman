use std::collections::HashMap;

use anyhow::{Context, Result};
use sqlx::Row;
use uuid::Uuid;

use crate::error::ApiError;
use crate::model::*;
use crate::repository::Repository;
use crate::state::OperatorAuthThrottleConfig;
use crate::{
    generate_token, hash_operator_password, normalize_operator_scopes, token_hash, unix_now,
    verify_operator_password, ACCESS_TOKEN_TTL_SECS, DEFAULT_REFRESH_TOKEN_TTL_SECS,
    MAX_REFRESH_TOKEN_TTL_SECS, MIN_REFRESH_TOKEN_TTL_SECS,
};

#[derive(Debug)]
pub(crate) enum OperatorLoginAttempt {
    Authenticated(Box<AuthResponse>),
    InvalidCredentials,
    Throttled,
}

#[derive(Clone, Copy)]
enum OperatorLoginFailureReason {
    UnknownUser,
    Disabled,
    Deleted,
    BadPassword,
    MissingTotp,
    MissingTotpSecret,
    TotpDecryptFailed,
    BadTotp,
    OperatorStateChanged,
    TotpManagement,
}

#[derive(Clone, Copy)]
struct SuccessfulOperatorAuthContext<'a> {
    username_key: &'a str,
    attempted_username: &'a str,
    remote_ip: &'a str,
    user_agent: Option<&'a str>,
    cleared_previous_failures: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct OperatorBatchAuthoritySnapshot {
    pub(crate) operator_id: Uuid,
    pub(crate) status: String,
    pub(crate) role: String,
}

#[derive(Clone, Debug)]
pub(crate) struct OperatorSessionBatchAuthoritySnapshot {
    pub(crate) session_id: Uuid,
    pub(crate) operator_role: String,
}

#[derive(Clone, Debug)]
pub(crate) enum AccessBatchMutationOutcome<T> {
    Applied { target_id: Uuid, result: T },
    Rejected { target_id: Uuid, code: &'static str },
}

impl OperatorLoginFailureReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::UnknownUser => "unknown_user",
            Self::Disabled => "operator_disabled",
            Self::Deleted => "operator_deleted",
            Self::BadPassword => "bad_password",
            Self::MissingTotp => "missing_totp",
            Self::MissingTotpSecret => "missing_totp_secret",
            Self::TotpDecryptFailed => "totp_decrypt_failed",
            Self::BadTotp => "bad_totp",
            Self::OperatorStateChanged => "operator_state_changed",
            Self::TotpManagement => "totp_management_invalid_credentials",
        }
    }
}

#[derive(Clone, Debug)]
struct AuthThrottleLockout {
    scope_kind: &'static str,
    scope_key: String,
    failed_attempts: i64,
}

impl Repository {
    pub(crate) async fn operator_count(&self) -> Result<i64> {
        match self {
            Self::Postgres(pool) => {
                let row = sqlx::query("SELECT count(*) AS count FROM operators")
                    .fetch_one(pool)
                    .await?;
                Ok(row.try_get("count")?)
            }
        }
    }

    #[cfg(test)]
    pub(crate) async fn bootstrap_operator(
        &self,
        request: &BootstrapOperatorRequest,
    ) -> Result<AuthResponse> {
        self.bootstrap_operator_with_origin(request, None, None)
            .await
    }

    pub(crate) async fn bootstrap_operator_with_auth_event(
        &self,
        request: &BootstrapOperatorRequest,
        remote_ip: &str,
        user_agent: Option<&str>,
    ) -> Result<AuthResponse> {
        self.bootstrap_operator_with_origin(request, Some(remote_ip), user_agent)
            .await
    }

    async fn bootstrap_operator_with_origin(
        &self,
        request: &BootstrapOperatorRequest,
        remote_ip: Option<&str>,
        user_agent: Option<&str>,
    ) -> Result<AuthResponse> {
        let now = unix_now().to_string();
        let operator = OperatorRecord {
            id: Uuid::new_v4(),
            username: request.username.trim().to_string(),
            password_hash: hash_operator_password(&request.password)?,
            status: "active".to_string(),
            role: "admin".to_string(),
            scopes: normalize_operator_scopes("admin", &[])
                .map_err(|error| anyhow::anyhow!(error.code))?,
            preferences: crate::model::OperatorPreferences::default(),
            totp_enabled: false,
            totp_secret_ciphertext_hex: None,
            totp_secret_nonce_hex: None,
            totp_secret_salt_hex: None,
            totp_last_accepted_step: None,
            session_refresh_ttl_secs: DEFAULT_REFRESH_TOKEN_TTL_SECS,
            created_at: now,
            disabled_at: None,
            deleted_at: None,
        };
        let session = PreparedOperatorSession::new(operator.session_refresh_ttl_secs);
        let auth_event = remote_ip.map(|remote_ip| {
            PreparedOperatorAuthEvent::new(
                Some((
                    operator.id,
                    operator.username.as_str(),
                    operator.role.as_str(),
                )),
                &operator.username,
                "success",
                None,
                remote_ip,
                user_agent,
                Some(session.session_id),
                false,
            )
        });
        match self {
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                sqlx::query("SELECT pg_advisory_xact_lock(hashtext('vpsman.bootstrap_operator'))")
                    .execute(&mut *tx)
                    .await?;
                let row = sqlx::query("SELECT count(*) AS count FROM operators")
                    .fetch_one(&mut *tx)
                    .await?;
                let operator_count: i64 = row.try_get("count")?;
                if operator_count > 0 {
                    anyhow::bail!("operator_already_bootstrapped");
                }
                sqlx::query(
                    r#"
                    INSERT INTO operators (
                        id, username, password_hash, status, role, scopes,
                        preferences, session_refresh_ttl_secs
                    )
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                    "#,
                )
                .bind(operator.id)
                .bind(&operator.username)
                .bind(&operator.password_hash)
                .bind(&operator.status)
                .bind(&operator.role)
                .bind(serde_json::json!(operator.scopes))
                .bind(serde_json::json!(operator.preferences))
                .bind(operator.session_refresh_ttl_secs as i64)
                .execute(&mut *tx)
                .await?;
                insert_operator_session_in_tx(&mut tx, operator.id, &session).await?;
                if let Some(auth_event) = &auth_event {
                    insert_operator_auth_event_in_tx(&mut tx, auth_event).await?;
                }
                tx.commit().await?;
                Ok(session.auth_response(operator.view()))
            }
        }
    }

    pub(crate) async fn login_operator_with_throttle(
        &self,
        request: &LoginRequest,
        remote_ip: &str,
        user_agent: Option<&str>,
        throttle: &OperatorAuthThrottleConfig,
    ) -> Result<OperatorLoginAttempt> {
        let username_key = normalize_auth_throttle_identity(&request.username, remote_ip);
        let ip_key = normalize_auth_throttle_ip(remote_ip);
        if self
            .operator_auth_throttle_locked(&username_key, &ip_key)
            .await?
        {
            return Ok(OperatorLoginAttempt::Throttled);
        }

        let Some(operator) = self.operator_by_username(&request.username).await? else {
            self.record_operator_auth_event(
                None,
                request.username.trim(),
                "failure",
                Some(OperatorLoginFailureReason::UnknownUser.as_str()),
                &ip_key,
                user_agent,
                None,
                false,
            )
            .await?;
            self.record_operator_auth_failure(
                &username_key,
                &ip_key,
                OperatorLoginFailureReason::UnknownUser,
                throttle,
            )
            .await?;
            return Ok(OperatorLoginAttempt::InvalidCredentials);
        };
        if operator.status != "active" {
            let reason = if operator.status == "deleted" {
                OperatorLoginFailureReason::Deleted
            } else {
                OperatorLoginFailureReason::Disabled
            };
            self.record_operator_auth_event(
                Some(&operator),
                request.username.trim(),
                "failure",
                Some(reason.as_str()),
                &ip_key,
                user_agent,
                None,
                false,
            )
            .await?;
            self.record_operator_auth_failure(&username_key, &ip_key, reason, throttle)
                .await?;
            return Ok(OperatorLoginAttempt::InvalidCredentials);
        }
        if !verify_operator_password(&request.password, &operator.password_hash)? {
            self.record_operator_auth_event(
                Some(&operator),
                request.username.trim(),
                "failure",
                Some(OperatorLoginFailureReason::BadPassword.as_str()),
                &ip_key,
                user_agent,
                None,
                false,
            )
            .await?;
            self.record_operator_auth_failure(
                &username_key,
                &ip_key,
                OperatorLoginFailureReason::BadPassword,
                throttle,
            )
            .await?;
            return Ok(OperatorLoginAttempt::InvalidCredentials);
        }
        let matched_totp_step = if operator.totp_enabled {
            let Some(code) = request.totp_code.as_deref() else {
                self.record_operator_auth_event(
                    Some(&operator),
                    request.username.trim(),
                    "failure",
                    Some(OperatorLoginFailureReason::MissingTotp.as_str()),
                    &ip_key,
                    user_agent,
                    None,
                    false,
                )
                .await?;
                self.record_operator_auth_failure(
                    &username_key,
                    &ip_key,
                    OperatorLoginFailureReason::MissingTotp,
                    throttle,
                )
                .await?;
                return Ok(OperatorLoginAttempt::InvalidCredentials);
            };
            let Some(secret) = operator.encrypted_totp_secret() else {
                self.record_operator_auth_event(
                    Some(&operator),
                    request.username.trim(),
                    "failure",
                    Some(OperatorLoginFailureReason::MissingTotpSecret.as_str()),
                    &ip_key,
                    user_agent,
                    None,
                    false,
                )
                .await?;
                self.record_operator_auth_failure(
                    &username_key,
                    &ip_key,
                    OperatorLoginFailureReason::MissingTotpSecret,
                    throttle,
                )
                .await?;
                return Ok(OperatorLoginAttempt::InvalidCredentials);
            };
            let secret = match crate::auth_totp::decrypt_totp_secret(&request.password, &secret) {
                Ok(secret) => secret,
                Err(_) => {
                    self.record_operator_auth_event(
                        Some(&operator),
                        request.username.trim(),
                        "failure",
                        Some(OperatorLoginFailureReason::TotpDecryptFailed.as_str()),
                        &ip_key,
                        user_agent,
                        None,
                        false,
                    )
                    .await?;
                    self.record_operator_auth_failure(
                        &username_key,
                        &ip_key,
                        OperatorLoginFailureReason::TotpDecryptFailed,
                        throttle,
                    )
                    .await?;
                    return Ok(OperatorLoginAttempt::InvalidCredentials);
                }
            };
            let matched_step = crate::auth_totp::matching_totp_step(&secret, code, unix_now())
                .filter(|step| {
                    operator
                        .totp_last_accepted_step
                        .is_none_or(|last_step| *step > last_step)
                });
            let Some(matched_step) = matched_step else {
                self.record_operator_auth_event(
                    Some(&operator),
                    request.username.trim(),
                    "failure",
                    Some(OperatorLoginFailureReason::BadTotp.as_str()),
                    &ip_key,
                    user_agent,
                    None,
                    false,
                )
                .await?;
                self.record_operator_auth_failure(
                    &username_key,
                    &ip_key,
                    OperatorLoginFailureReason::BadTotp,
                    throttle,
                )
                .await?;
                return Ok(OperatorLoginAttempt::InvalidCredentials);
            };
            Some(matched_step)
        } else {
            None
        };
        let previous_failures = self
            .operator_auth_previous_failures(&username_key, throttle)
            .await?;
        let success_context = SuccessfulOperatorAuthContext {
            username_key: &username_key,
            attempted_username: request.username.trim(),
            remote_ip: &ip_key,
            user_agent,
            cleared_previous_failures: previous_failures,
        };
        let response = match matched_totp_step {
            Some(step) => {
                let Some(response) = self
                    .issue_totp_login_session(&operator, step, Some(success_context))
                    .await?
                else {
                    self.record_operator_auth_event(
                        Some(&operator),
                        request.username.trim(),
                        "failure",
                        Some(OperatorLoginFailureReason::BadTotp.as_str()),
                        &ip_key,
                        user_agent,
                        None,
                        false,
                    )
                    .await?;
                    self.record_operator_auth_failure(
                        &username_key,
                        &ip_key,
                        OperatorLoginFailureReason::BadTotp,
                        throttle,
                    )
                    .await?;
                    return Ok(OperatorLoginAttempt::InvalidCredentials);
                };
                response
            }
            None => {
                let Some(response) = self
                    .issue_password_login_session(&operator, success_context)
                    .await?
                else {
                    self.record_operator_auth_event(
                        Some(&operator),
                        request.username.trim(),
                        "failure",
                        Some(OperatorLoginFailureReason::OperatorStateChanged.as_str()),
                        &ip_key,
                        user_agent,
                        None,
                        false,
                    )
                    .await?;
                    self.record_operator_auth_failure(
                        &username_key,
                        &ip_key,
                        OperatorLoginFailureReason::OperatorStateChanged,
                        throttle,
                    )
                    .await?;
                    return Ok(OperatorLoginAttempt::InvalidCredentials);
                };
                response
            }
        };
        Ok(OperatorLoginAttempt::Authenticated(Box::new(response)))
    }

    pub(crate) async fn operator_auth_identity_locked(
        &self,
        username: &str,
        remote_ip: &str,
    ) -> Result<bool> {
        let username_key = normalize_auth_throttle_identity(username, remote_ip);
        let ip_key = normalize_auth_throttle_ip(remote_ip);
        self.operator_auth_throttle_locked(&username_key, &ip_key)
            .await
    }

    pub(crate) async fn record_operator_totp_management_failure(
        &self,
        username: &str,
        remote_ip: &str,
        throttle: &OperatorAuthThrottleConfig,
    ) -> Result<()> {
        let username_key = normalize_auth_throttle_identity(username, remote_ip);
        let ip_key = normalize_auth_throttle_ip(remote_ip);
        self.record_operator_auth_failure(
            &username_key,
            &ip_key,
            OperatorLoginFailureReason::TotpManagement,
            throttle,
        )
        .await
    }

    pub(crate) async fn clear_operator_auth_management_success(
        &self,
        username: &str,
        remote_ip: &str,
    ) -> Result<()> {
        let username_key = normalize_auth_throttle_identity(username, remote_ip);
        self.clear_operator_auth_success(&username_key).await
    }

    async fn operator_auth_throttle_locked(
        &self,
        username_key: &str,
        ip_key: &str,
    ) -> Result<bool> {
        match self {
            Self::Postgres(pool) => {
                let row = sqlx::query(
                    r#"
                    SELECT EXISTS (
                        SELECT 1
                        FROM operator_auth_throttle
                        WHERE (
                            (scope_kind = 'username_ip' AND scope_key = $1)
                            OR (scope_kind = 'ip' AND scope_key = $2)
                        )
                          AND locked_until IS NOT NULL
                          AND locked_until > now()
                    ) AS locked
                    "#,
                )
                .bind(username_key)
                .bind(ip_key)
                .fetch_one(pool)
                .await?;
                Ok(row.try_get("locked")?)
            }
        }
    }

    async fn record_operator_auth_failure(
        &self,
        username_key: &str,
        ip_key: &str,
        reason: OperatorLoginFailureReason,
        throttle: &OperatorAuthThrottleConfig,
    ) -> Result<()> {
        match self {
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let mut lockouts = Vec::new();
                if let Some(lockout) = record_postgres_throttle_failure(
                    &mut tx,
                    "username_ip",
                    username_key,
                    throttle.username_failed_attempt_limit,
                    throttle.failed_attempt_window_secs,
                    throttle.lockout_secs,
                    reason.as_str(),
                )
                .await?
                {
                    lockouts.push(lockout);
                }
                if let Some(lockout) = record_postgres_throttle_failure(
                    &mut tx,
                    "ip",
                    ip_key,
                    throttle.ip_failed_attempt_limit,
                    throttle.failed_attempt_window_secs,
                    throttle.lockout_secs,
                    reason.as_str(),
                )
                .await?
                {
                    lockouts.push(lockout);
                }
                for lockout in &lockouts {
                    insert_postgres_auth_lockout_audit(&mut tx, lockout, reason.as_str(), ip_key)
                        .await?;
                }
                tx.commit().await?;
                Ok(())
            }
        }
    }

    async fn operator_auth_previous_failures(
        &self,
        username_key: &str,
        throttle: &OperatorAuthThrottleConfig,
    ) -> Result<bool> {
        match self {
            Self::Postgres(pool) => {
                let row = sqlx::query(
                    r#"
                    SELECT EXISTS (
                        SELECT 1
                        FROM operator_auth_throttle
                        WHERE (
                            (scope_kind = 'username_ip' AND scope_key = $1)
                        )
                          AND failed_attempts > 0
                          AND (
                            window_started_at + make_interval(secs => $2::double precision) > now()
                            OR (locked_until IS NOT NULL AND locked_until > now())
                          )
                    ) AS has_failures
                    "#,
                )
                .bind(username_key)
                .bind(throttle.failed_attempt_window_secs as f64)
                .fetch_one(pool)
                .await?;
                Ok(row.try_get("has_failures")?)
            }
        }
    }

    async fn clear_operator_auth_success(&self, username_key: &str) -> Result<()> {
        match self {
            Self::Postgres(pool) => {
                sqlx::query(
                    "DELETE FROM operator_auth_throttle WHERE scope_kind = 'username_ip' AND scope_key = $1",
                )
                .bind(username_key)
                .execute(pool)
                .await?;
                Ok(())
            }
        }
    }

    async fn record_operator_auth_event(
        &self,
        operator: Option<&OperatorRecord>,
        attempted_username: &str,
        result: &str,
        reason: Option<&str>,
        remote_ip: &str,
        user_agent: Option<&str>,
        session_id: Option<Uuid>,
        cleared_previous_failures: bool,
    ) -> Result<()> {
        self.record_operator_auth_event_for_identity(
            operator.map(|operator| {
                (
                    operator.id,
                    operator.username.as_str(),
                    operator.role.as_str(),
                )
            }),
            attempted_username,
            result,
            reason,
            remote_ip,
            user_agent,
            session_id,
            cleared_previous_failures,
        )
        .await
    }

    async fn record_operator_auth_event_for_identity(
        &self,
        operator: Option<(Uuid, &str, &str)>,
        attempted_username: &str,
        result: &str,
        reason: Option<&str>,
        remote_ip: &str,
        user_agent: Option<&str>,
        session_id: Option<Uuid>,
        cleared_previous_failures: bool,
    ) -> Result<()> {
        let event = PreparedOperatorAuthEvent::new(
            operator,
            attempted_username,
            result,
            reason,
            remote_ip,
            user_agent,
            session_id,
            cleared_previous_failures,
        );
        match self {
            Self::Postgres(pool) => {
                insert_operator_auth_event(pool, &event).await?;
            }
        }
        Ok(())
    }

    pub(crate) async fn refresh_operator_session(
        &self,
        refresh_token: &str,
    ) -> Result<Option<AuthResponse>> {
        let refresh_hash = token_hash(refresh_token);
        match self {
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let row = sqlx::query(
                    r#"
                    SELECT
                        o.id,
                        o.username,
                        o.status,
                        o.role,
                        o.scopes,
                        o.preferences,
                        o.totp_enabled,
                        o.session_refresh_ttl_secs,
                        o.created_at::text AS created_at,
                        o.disabled_at::text AS disabled_at,
                        o.deleted_at::text AS deleted_at
                    FROM operator_sessions s
                    JOIN operators o ON o.id = s.operator_id
                    WHERE s.refresh_token_hash = $1
                      AND s.refresh_expires_at > now()
                      AND s.revoked_at IS NULL
                      AND o.status = 'active'
                    FOR UPDATE OF o
                    "#,
                )
                .bind(&refresh_hash)
                .fetch_optional(&mut *tx)
                .await?;
                let Some(row) = row else {
                    tx.rollback().await?;
                    return Ok(None);
                };
                let operator = operator_view_from_row(&row)?;
                let revoked = sqlx::query(
                    r#"
                    UPDATE operator_sessions
                    SET revoked_at = now()
                    WHERE refresh_token_hash = $1
                      AND operator_id = $2
                      AND refresh_expires_at > now()
                      AND revoked_at IS NULL
                    "#,
                )
                .bind(&refresh_hash)
                .bind(operator.id)
                .execute(&mut *tx)
                .await?;
                if revoked.rows_affected() == 0 {
                    tx.rollback().await?;
                    return Ok(None);
                }
                let replacement = PreparedOperatorSession::new(operator.session_refresh_ttl_secs);
                insert_operator_session_in_tx(&mut tx, operator.id, &replacement).await?;
                tx.commit().await?;
                Ok(Some(replacement.auth_response(operator)))
            }
        }
    }

    pub(crate) async fn authenticate_access_token(
        &self,
        access_token: &str,
    ) -> Result<Option<AuthContext>> {
        let access_hash = token_hash(access_token);
        match self {
            Self::Postgres(pool) => {
                let row = sqlx::query(
                    r#"
                    SELECT
                        s.id AS session_id,
                        o.id AS operator_id,
                        o.username,
                        o.status,
                        o.role,
                        o.scopes,
                        o.preferences,
                        o.totp_enabled,
                        o.session_refresh_ttl_secs,
                        o.created_at::text AS created_at,
                        o.disabled_at::text AS disabled_at,
                        o.deleted_at::text AS deleted_at
                    FROM operator_sessions s
                    JOIN operators o ON o.id = s.operator_id
                    WHERE s.access_token_hash = $1
                      AND s.expires_at > now()
                      AND s.revoked_at IS NULL
                      AND o.status = 'active'
                    "#,
                )
                .bind(&access_hash)
                .fetch_optional(pool)
                .await?;
                row.map(|row| {
                    Ok(AuthContext {
                        session_id: Some(row.try_get("session_id")?),
                        operator: OperatorView {
                            id: row.try_get("operator_id")?,
                            username: row.try_get("username")?,
                            status: row.try_get("status")?,
                            role: row.try_get("role")?,
                            scopes: parse_scopes(row.try_get("scopes")?),
                            preferences: parse_operator_preferences(row.try_get("preferences")?),
                            totp_enabled: row.try_get("totp_enabled")?,
                            session_refresh_ttl_secs: row
                                .try_get::<i64, _>("session_refresh_ttl_secs")?
                                .try_into()
                                .unwrap_or(DEFAULT_REFRESH_TOKEN_TTL_SECS),
                            created_at: row.try_get("created_at")?,
                            disabled_at: row.try_get("disabled_at")?,
                            deleted_at: row.try_get("deleted_at")?,
                        },
                    })
                })
                .transpose()
            }
        }
    }

    pub(crate) async fn operator_batch_authority_snapshots(
        &self,
        operator_ids: &[Uuid],
    ) -> Result<Vec<OperatorBatchAuthoritySnapshot>> {
        match self {
            Self::Postgres(pool) => sqlx::query(
                r#"
                SELECT id, status, role
                FROM operators
                WHERE id = ANY($1)
                ORDER BY id
                "#,
            )
            .bind(operator_ids)
            .fetch_all(pool)
            .await?
            .into_iter()
            .map(|row| {
                Ok(OperatorBatchAuthoritySnapshot {
                    operator_id: row.try_get("id")?,
                    status: row.try_get("status")?,
                    role: row.try_get("role")?,
                })
            })
            .collect(),
        }
    }

    pub(crate) async fn operator_session_batch_authority_snapshots(
        &self,
        session_ids: &[Uuid],
    ) -> Result<Vec<OperatorSessionBatchAuthoritySnapshot>> {
        match self {
            Self::Postgres(pool) => sqlx::query(
                r#"
                SELECT session.id, operator.role AS operator_role
                FROM operator_sessions AS session
                JOIN operators AS operator ON operator.id = session.operator_id
                WHERE session.id = ANY($1)
                ORDER BY session.id
                "#,
            )
            .bind(session_ids)
            .fetch_all(pool)
            .await?
            .into_iter()
            .map(|row| {
                Ok(OperatorSessionBatchAuthoritySnapshot {
                    session_id: row.try_get("id")?,
                    operator_role: row.try_get("operator_role")?,
                })
            })
            .collect(),
        }
    }

    pub(crate) async fn operator_by_id(&self, id: Uuid) -> Result<Option<OperatorRecord>> {
        match self {
            Self::Postgres(pool) => {
                let row = sqlx::query(
                    r#"
                    SELECT
                        id,
                        username,
                        password_hash,
                        status,
                        role,
                        scopes,
                        preferences,
                        totp_enabled,
                        totp_secret_ciphertext_hex,
                        totp_secret_nonce_hex,
                        totp_secret_salt_hex,
                        totp_last_accepted_step,
                        session_refresh_ttl_secs,
                        created_at::text AS created_at,
                        disabled_at::text AS disabled_at,
                        deleted_at::text AS deleted_at
                    FROM operators
                    WHERE id = $1
                    "#,
                )
                .bind(id)
                .fetch_optional(pool)
                .await?;
                row.map(|row| {
                    Ok(OperatorRecord {
                        id: row.try_get("id")?,
                        username: row.try_get("username")?,
                        password_hash: row.try_get("password_hash")?,
                        status: row.try_get("status")?,
                        role: row.try_get("role")?,
                        scopes: parse_scopes(row.try_get("scopes")?),
                        preferences: parse_operator_preferences(row.try_get("preferences")?),
                        totp_enabled: row.try_get("totp_enabled")?,
                        totp_secret_ciphertext_hex: row.try_get("totp_secret_ciphertext_hex")?,
                        totp_secret_nonce_hex: row.try_get("totp_secret_nonce_hex")?,
                        totp_secret_salt_hex: row.try_get("totp_secret_salt_hex")?,
                        totp_last_accepted_step: postgres_totp_step(&row)?,
                        session_refresh_ttl_secs: row
                            .try_get::<i64, _>("session_refresh_ttl_secs")?
                            .try_into()
                            .unwrap_or(DEFAULT_REFRESH_TOKEN_TTL_SECS),
                        created_at: row.try_get("created_at")?,
                        disabled_at: row.try_get("disabled_at")?,
                        deleted_at: row.try_get("deleted_at")?,
                    })
                })
                .transpose()
            }
        }
    }

    pub(crate) async fn operator_by_username(
        &self,
        username: &str,
    ) -> Result<Option<OperatorRecord>> {
        let username = username.trim();
        match self {
            Self::Postgres(pool) => {
                let row = sqlx::query(
                    r#"
                    SELECT
                        id,
                        username,
                        password_hash,
                        status,
                        role,
                        scopes,
                        preferences,
                        totp_enabled,
                        totp_secret_ciphertext_hex,
                        totp_secret_nonce_hex,
                        totp_secret_salt_hex,
                        totp_last_accepted_step,
                        session_refresh_ttl_secs,
                        created_at::text AS created_at,
                        disabled_at::text AS disabled_at,
                        deleted_at::text AS deleted_at
                    FROM operators
                    WHERE username = $1
                    "#,
                )
                .bind(username)
                .fetch_optional(pool)
                .await?;
                row.map(|row| {
                    Ok(OperatorRecord {
                        id: row.try_get("id")?,
                        username: row.try_get("username")?,
                        password_hash: row.try_get("password_hash")?,
                        status: row.try_get("status")?,
                        role: row.try_get("role")?,
                        scopes: parse_scopes(row.try_get("scopes")?),
                        preferences: parse_operator_preferences(row.try_get("preferences")?),
                        totp_enabled: row.try_get("totp_enabled")?,
                        totp_secret_ciphertext_hex: row.try_get("totp_secret_ciphertext_hex")?,
                        totp_secret_nonce_hex: row.try_get("totp_secret_nonce_hex")?,
                        totp_secret_salt_hex: row.try_get("totp_secret_salt_hex")?,
                        totp_last_accepted_step: postgres_totp_step(&row)?,
                        session_refresh_ttl_secs: row
                            .try_get::<i64, _>("session_refresh_ttl_secs")?
                            .try_into()
                            .unwrap_or(DEFAULT_REFRESH_TOKEN_TTL_SECS),
                        created_at: row.try_get("created_at")?,
                        disabled_at: row.try_get("disabled_at")?,
                        deleted_at: row.try_get("deleted_at")?,
                    })
                })
                .transpose()
            }
        }
    }

    pub(crate) async fn list_operators(&self) -> Result<Vec<OperatorView>> {
        match self {
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        id,
                        username,
                        status,
                        role,
                        scopes,
                        preferences,
                        totp_enabled,
                        session_refresh_ttl_secs,
                        created_at::text AS created_at,
                        disabled_at::text AS disabled_at,
                        deleted_at::text AS deleted_at
                    FROM operators
                    ORDER BY created_at ASC, username ASC
                    "#,
                )
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(|row| {
                        Ok(OperatorView {
                            id: row.try_get("id")?,
                            username: row.try_get("username")?,
                            status: row.try_get("status")?,
                            role: row.try_get("role")?,
                            scopes: parse_scopes(row.try_get("scopes")?),
                            preferences: parse_operator_preferences(row.try_get("preferences")?),
                            totp_enabled: row.try_get("totp_enabled")?,
                            session_refresh_ttl_secs: row
                                .try_get::<i64, _>("session_refresh_ttl_secs")?
                                .try_into()
                                .unwrap_or(DEFAULT_REFRESH_TOKEN_TTL_SECS),
                            created_at: row.try_get("created_at")?,
                            disabled_at: row.try_get("disabled_at")?,
                            deleted_at: row.try_get("deleted_at")?,
                        })
                    })
                    .collect()
            }
        }
    }

    pub(crate) async fn create_operator(
        &self,
        request: &CreateOperatorRequest,
        actor: &AuthContext,
    ) -> Result<OperatorView> {
        let username = request.username.trim().to_string();
        let role = request.role.trim().to_string();
        let scopes = normalize_operator_scopes(&role, &request.scopes)
            .map_err(|error| anyhow::anyhow!(error.code))?;
        let session_refresh_ttl_secs = normalize_session_refresh_ttl(
            request
                .session_refresh_ttl_secs
                .unwrap_or(DEFAULT_REFRESH_TOKEN_TTL_SECS),
        )
        .map_err(|error| anyhow::anyhow!(error.code))?;
        let now = unix_now().to_string();
        let operator = OperatorRecord {
            id: Uuid::new_v4(),
            username,
            password_hash: hash_operator_password(&request.password)?,
            status: "active".to_string(),
            role,
            scopes,
            preferences: crate::model::OperatorPreferences::default(),
            totp_enabled: false,
            totp_secret_ciphertext_hex: None,
            totp_secret_nonce_hex: None,
            totp_secret_salt_hex: None,
            totp_last_accepted_step: None,
            session_refresh_ttl_secs,
            created_at: now,
            disabled_at: None,
            deleted_at: None,
        };
        let metadata = serde_json::json!({
            "target_operator_id": operator.id,
            "target_operator_username": operator.username,
            "target_operator_role": operator.role,
            "target_operator_scopes": operator.scopes,
            "session_refresh_ttl_secs": operator.session_refresh_ttl_secs,
            "operator_id": actor.operator.id,
            "operator_username": actor.operator.username,
            "operator_role": actor.operator.role,
            "operator_session_id": actor.audit_session_id(),
            "result": "succeeded",
            "origin_kind": "operator_request",
            "component": "operator-admin-controller",
        });
        match self {
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                sqlx::query(
                    r#"
                    INSERT INTO operators (
                        id, username, password_hash, status, role, scopes,
                        preferences, session_refresh_ttl_secs
                    )
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                    "#,
                )
                .bind(operator.id)
                .bind(&operator.username)
                .bind(&operator.password_hash)
                .bind(&operator.status)
                .bind(&operator.role)
                .bind(serde_json::json!(operator.scopes))
                .bind(serde_json::json!(operator.preferences))
                .bind(operator.session_refresh_ttl_secs as i64)
                .execute(&mut *tx)
                .await?;
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (
                        id, actor_id, action, target, command_hash, metadata
                    )
                    VALUES ($1, $2, $3, $4, NULL, $5)
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind(actor.operator.id)
                .bind("operator.created")
                .bind(format!("operator:{}", operator.id))
                .bind(metadata)
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
            }
        }
        Ok(operator.view())
    }

    pub(crate) async fn update_operator(
        &self,
        operator_id: Uuid,
        request: &UpdateOperatorRequest,
        actor: &AuthContext,
    ) -> Result<Option<OperatorView>> {
        let role = request.role.trim().to_string();
        let scopes = normalize_operator_scopes(&role, &request.scopes)
            .map_err(|error| anyhow::anyhow!(error.code))?;
        let session_refresh_ttl_secs =
            normalize_session_refresh_ttl(request.session_refresh_ttl_secs)
                .map_err(|error| anyhow::anyhow!(error.code))?;
        let metadata = serde_json::json!({
            "target_operator_id": operator_id,
            "target_operator_role": role,
            "target_operator_scopes": scopes,
            "session_refresh_ttl_secs": session_refresh_ttl_secs,
            "operator_id": actor.operator.id,
            "operator_username": actor.operator.username,
            "operator_role": actor.operator.role,
            "operator_session_id": actor.audit_session_id(),
            "result": "succeeded",
            "origin_kind": "operator_request",
            "component": "operator-admin-controller",
        });
        match self {
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                lock_postgres_active_admin_invariant(&mut tx).await?;
                let target = sqlx::query(
                    "SELECT status, role FROM operators WHERE id = $1 AND status <> 'deleted' FOR UPDATE",
                )
                .bind(operator_id)
                .fetch_optional(&mut *tx)
                .await?;
                let Some(target) = target else {
                    return Ok(None);
                };
                ensure_postgres_active_admin_remains(
                    &mut tx,
                    operator_id,
                    target.try_get("status")?,
                    target.try_get("role")?,
                    Some(&role),
                    None,
                )
                .await?;
                let row = sqlx::query(
                    r#"
                    UPDATE operators
                    SET role = $2,
                        scopes = $3,
                        session_refresh_ttl_secs = $4
                    WHERE id = $1 AND status <> 'deleted'
                    RETURNING
                        id, username, status, role, scopes, preferences,
                        totp_enabled, session_refresh_ttl_secs,
                        created_at::text AS created_at,
                        disabled_at::text AS disabled_at,
                        deleted_at::text AS deleted_at
                    "#,
                )
                .bind(operator_id)
                .bind(&role)
                .bind(serde_json::json!(scopes))
                .bind(session_refresh_ttl_secs as i64)
                .fetch_optional(&mut *tx)
                .await?;
                let Some(row) = row else {
                    return Ok(None);
                };
                sqlx::query(audit_insert_sql())
                    .bind(Uuid::new_v4())
                    .bind(actor.operator.id)
                    .bind("operator.updated")
                    .bind(format!("operator:{operator_id}"))
                    .bind(metadata)
                    .execute(&mut *tx)
                    .await?;
                let operator = operator_view_from_row(&row)?;
                tx.commit().await?;
                Ok(Some(operator))
            }
        }
    }

    pub(crate) async fn set_operator_statuses(
        &self,
        operator_ids: &[Uuid],
        status: &str,
        actor: &AuthContext,
    ) -> Result<Vec<AccessBatchMutationOutcome<OperatorView>>> {
        let status = status.trim();
        if !matches!(status, "active" | "disabled" | "deleted") {
            anyhow::bail!("invalid_operator_status");
        }
        let action = match status {
            "active" => "operator.enabled",
            "disabled" => "operator.disabled",
            "deleted" => "operator.deleted",
            _ => unreachable!(),
        };
        match self {
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                lock_postgres_active_admin_invariant(&mut tx).await?;
                let locked_rows = sqlx::query(
                    r#"
                    SELECT
                        id, username, status, role, scopes, preferences,
                        totp_enabled, session_refresh_ttl_secs,
                        created_at::text AS created_at,
                        disabled_at::text AS disabled_at,
                        deleted_at::text AS deleted_at
                    FROM operators
                    WHERE id = ANY($1)
                    ORDER BY id
                    FOR UPDATE
                    "#,
                )
                .bind(operator_ids)
                .fetch_all(&mut *tx)
                .await?;
                let locked = locked_rows
                    .iter()
                    .map(|row| Ok((row.try_get::<Uuid, _>("id")?, operator_view_from_row(row)?)))
                    .collect::<Result<HashMap<_, _>>>()?;
                let mut active_admin_count = sqlx::query_scalar::<_, i64>(
                    "SELECT count(*) FROM operators WHERE status = 'active' AND role = 'admin'",
                )
                .fetch_one(&mut *tx)
                .await?;
                let mut applied_ids = Vec::with_capacity(operator_ids.len());
                let mut rejection_codes = HashMap::new();
                for operator_id in operator_ids {
                    let Some(operator) = locked.get(operator_id) else {
                        rejection_codes.insert(*operator_id, "operator_not_found");
                        continue;
                    };
                    if operator.status == "deleted"
                        || (status == "active" && operator.status != "disabled")
                    {
                        rejection_codes.insert(*operator_id, "operator_not_found");
                        continue;
                    }
                    if status != "active" && operator.status == "active" && operator.role == "admin"
                    {
                        if active_admin_count <= 1 {
                            rejection_codes.insert(*operator_id, "last_active_admin_required");
                            continue;
                        }
                        active_admin_count -= 1;
                    }
                    applied_ids.push(*operator_id);
                }
                if applied_ids.is_empty() {
                    tx.rollback().await?;
                    return Ok(operator_ids
                        .iter()
                        .map(|operator_id| AccessBatchMutationOutcome::Rejected {
                            target_id: *operator_id,
                            code: rejection_codes
                                .get(operator_id)
                                .copied()
                                .unwrap_or("operator_not_found"),
                        })
                        .collect());
                }
                let updated_rows = sqlx::query(
                    r#"
                    UPDATE operators
                    SET status = $2,
                        disabled_at = CASE
                            WHEN $2 = 'active' THEN NULL
                            WHEN disabled_at IS NULL THEN now()
                            ELSE disabled_at
                        END,
                        deleted_at = CASE
                            WHEN $2 = 'deleted' AND deleted_at IS NULL THEN now()
                            ELSE deleted_at
                        END
                    WHERE id = ANY($1)
                    RETURNING
                        id, username, status, role, scopes, preferences,
                        totp_enabled, session_refresh_ttl_secs,
                        created_at::text AS created_at,
                        disabled_at::text AS disabled_at,
                        deleted_at::text AS deleted_at
                    "#,
                )
                .bind(&applied_ids)
                .bind(status)
                .fetch_all(&mut *tx)
                .await?;
                if status != "active" {
                    sqlx::query(
                        "UPDATE operator_sessions SET revoked_at = COALESCE(revoked_at, now()) WHERE operator_id = ANY($1)",
                    )
                    .bind(&applied_ids)
                    .execute(&mut *tx)
                    .await?;
                }
                let audit_ids = applied_ids
                    .iter()
                    .map(|_| Uuid::new_v4())
                    .collect::<Vec<_>>();
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (id, actor_id, action, target, command_hash, metadata)
                    SELECT
                        mutation.audit_id,
                        $3,
                        $4,
                        'operator:' || mutation.operator_id::text,
                        NULL,
                        jsonb_build_object(
                            'target_operator_id', mutation.operator_id,
                            'target_operator_status', $5::text,
                            'operator_id', $3::uuid,
                            'operator_username', $6::text,
                            'operator_role', $7::text,
                            'operator_session_id', $8::uuid,
                            'result', 'succeeded',
                            'origin_kind', 'operator_request',
                            'component', 'operator-admin-controller'
                        )
                    FROM unnest($1::uuid[], $2::uuid[])
                        AS mutation(audit_id, operator_id)
                    "#,
                )
                .bind(&audit_ids)
                .bind(&applied_ids)
                .bind(actor.operator.id)
                .bind(action)
                .bind(status)
                .bind(&actor.operator.username)
                .bind(&actor.operator.role)
                .bind(actor.audit_session_id())
                .execute(&mut *tx)
                .await?;
                let updated = updated_rows
                    .iter()
                    .map(|row| Ok((row.try_get::<Uuid, _>("id")?, operator_view_from_row(row)?)))
                    .collect::<Result<HashMap<_, _>>>()?;
                let outcomes = operator_ids
                    .iter()
                    .map(|operator_id| {
                        if let Some(code) = rejection_codes.get(operator_id) {
                            Ok(AccessBatchMutationOutcome::Rejected {
                                target_id: *operator_id,
                                code,
                            })
                        } else {
                            Ok(AccessBatchMutationOutcome::Applied {
                                target_id: *operator_id,
                                result: updated
                                    .get(operator_id)
                                    .cloned()
                                    .with_context(|| {
                                        format!(
                                            "operator status update omitted approved target {operator_id}"
                                        )
                                    })?,
                            })
                        }
                    })
                    .collect::<Result<Vec<_>>>()?;
                tx.commit().await?;
                Ok(outcomes)
            }
        }
    }

    pub(crate) async fn reset_operator_password(
        &self,
        operator_id: Uuid,
        password: &str,
        actor: &AuthContext,
    ) -> Result<Option<OperatorView>> {
        let password_hash = hash_operator_password(password)?;
        let metadata = serde_json::json!({
            "target_operator_id": operator_id,
            "operator_id": actor.operator.id,
            "operator_username": actor.operator.username,
            "operator_role": actor.operator.role,
            "operator_session_id": actor.audit_session_id(),
            "sessions_revoked": true,
            "totp_cleared": true,
            "result": "succeeded",
            "origin_kind": "operator_request",
            "component": "operator-admin-controller",
        });
        match self {
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let row = sqlx::query(
                    r#"
                    UPDATE operators
                    SET password_hash = $2,
                        totp_enabled = FALSE,
                        totp_secret_ciphertext_hex = NULL,
                        totp_secret_nonce_hex = NULL,
                        totp_secret_salt_hex = NULL,
                        totp_last_accepted_step = NULL
                    WHERE id = $1 AND status <> 'deleted'
                    RETURNING
                        id, username, status, role, scopes, preferences,
                        totp_enabled, session_refresh_ttl_secs,
                        created_at::text AS created_at,
                        disabled_at::text AS disabled_at,
                        deleted_at::text AS deleted_at
                    "#,
                )
                .bind(operator_id)
                .bind(password_hash)
                .fetch_optional(&mut *tx)
                .await?;
                let Some(row) = row else {
                    return Ok(None);
                };
                sqlx::query(
                    "UPDATE operator_sessions SET revoked_at = COALESCE(revoked_at, now()) WHERE operator_id = $1",
                )
                .bind(operator_id)
                .execute(&mut *tx)
                .await?;
                sqlx::query(audit_insert_sql())
                    .bind(Uuid::new_v4())
                    .bind(actor.operator.id)
                    .bind("operator.password_reset")
                    .bind(format!("operator:{operator_id}"))
                    .bind(metadata)
                    .execute(&mut *tx)
                    .await?;
                let operator = operator_view_from_row(&row)?;
                tx.commit().await?;
                Ok(Some(operator))
            }
        }
    }

    pub(crate) async fn clear_operator_totps(
        &self,
        operator_ids: &[Uuid],
        actor: &AuthContext,
    ) -> Result<Vec<AccessBatchMutationOutcome<OperatorView>>> {
        match self {
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let locked_rows = sqlx::query(
                    r#"
                    SELECT
                        id, username, status, role, scopes, preferences,
                        totp_enabled, session_refresh_ttl_secs,
                        created_at::text AS created_at,
                        disabled_at::text AS disabled_at,
                        deleted_at::text AS deleted_at
                    FROM operators
                    WHERE id = ANY($1)
                    ORDER BY id
                    FOR UPDATE
                    "#,
                )
                .bind(operator_ids)
                .fetch_all(&mut *tx)
                .await?;
                let locked = locked_rows
                    .iter()
                    .map(|row| Ok((row.try_get::<Uuid, _>("id")?, operator_view_from_row(row)?)))
                    .collect::<Result<HashMap<_, _>>>()?;
                let mut applied_ids = Vec::with_capacity(operator_ids.len());
                let mut rejection_codes = HashMap::new();
                for operator_id in operator_ids {
                    match locked.get(operator_id) {
                        Some(operator) if operator.status != "deleted" => {
                            applied_ids.push(*operator_id)
                        }
                        _ => {
                            rejection_codes.insert(*operator_id, "operator_not_found");
                        }
                    }
                }
                if applied_ids.is_empty() {
                    tx.rollback().await?;
                    return Ok(operator_ids
                        .iter()
                        .map(|operator_id| AccessBatchMutationOutcome::Rejected {
                            target_id: *operator_id,
                            code: "operator_not_found",
                        })
                        .collect());
                }
                let updated_rows = sqlx::query(
                    r#"
                    UPDATE operators
                    SET totp_enabled = false,
                        totp_secret_ciphertext_hex = NULL,
                        totp_secret_nonce_hex = NULL,
                        totp_secret_salt_hex = NULL,
                        totp_last_accepted_step = NULL
                    WHERE id = ANY($1)
                    RETURNING
                        id, username, status, role, scopes, preferences,
                        totp_enabled, session_refresh_ttl_secs,
                        created_at::text AS created_at,
                        disabled_at::text AS disabled_at,
                        deleted_at::text AS deleted_at
                    "#,
                )
                .bind(&applied_ids)
                .fetch_all(&mut *tx)
                .await?;
                sqlx::query(
                    "UPDATE operator_sessions SET revoked_at = COALESCE(revoked_at, now()) WHERE operator_id = ANY($1)",
                )
                .bind(&applied_ids)
                .execute(&mut *tx)
                .await?;
                let audit_ids = applied_ids
                    .iter()
                    .map(|_| Uuid::new_v4())
                    .collect::<Vec<_>>();
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (id, actor_id, action, target, command_hash, metadata)
                    SELECT
                        mutation.audit_id,
                        $3,
                        'operator.totp_cleared',
                        'operator:' || mutation.operator_id::text,
                        NULL,
                        jsonb_build_object(
                            'target_operator_id', mutation.operator_id,
                            'operator_id', $3::uuid,
                            'operator_username', $4::text,
                            'operator_role', $5::text,
                            'operator_session_id', $6::uuid,
                            'sessions_revoked', true,
                            'result', 'succeeded',
                            'origin_kind', 'operator_request',
                            'component', 'operator-admin-controller'
                        )
                    FROM unnest($1::uuid[], $2::uuid[])
                        AS mutation(audit_id, operator_id)
                    "#,
                )
                .bind(&audit_ids)
                .bind(&applied_ids)
                .bind(actor.operator.id)
                .bind(&actor.operator.username)
                .bind(&actor.operator.role)
                .bind(actor.audit_session_id())
                .execute(&mut *tx)
                .await?;
                let updated = updated_rows
                    .iter()
                    .map(|row| Ok((row.try_get::<Uuid, _>("id")?, operator_view_from_row(row)?)))
                    .collect::<Result<HashMap<_, _>>>()?;
                let outcomes = operator_ids
                    .iter()
                    .map(|operator_id| {
                        if let Some(code) = rejection_codes.get(operator_id) {
                            Ok(AccessBatchMutationOutcome::Rejected {
                                target_id: *operator_id,
                                code,
                            })
                        } else {
                            Ok(AccessBatchMutationOutcome::Applied {
                                target_id: *operator_id,
                                result: updated.get(operator_id).cloned().with_context(|| {
                                    format!(
                                        "operator TOTP clear omitted approved target {operator_id}"
                                    )
                                })?,
                            })
                        }
                    })
                    .collect::<Result<Vec<_>>>()?;
                tx.commit().await?;
                Ok(outcomes)
            }
        }
    }

    pub(crate) async fn update_operator_preferences(
        &self,
        actor: &AuthContext,
        preferences: OperatorPreferences,
    ) -> Result<OperatorView> {
        let preferences = preferences.normalized();
        let metadata = serde_json::json!({
            "operator_id": actor.operator.id,
            "operator_username": actor.operator.username,
            "operator_role": actor.operator.role,
            "operator_session_id": actor.audit_session_id(),
            "preferences": preferences,
            "result": "succeeded",
            "origin_kind": "operator_request",
            "component": "operator-preferences-controller",
        });
        match self {
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let row = sqlx::query(
                    r#"
                    UPDATE operators
                    SET preferences = $2
                    WHERE id = $1
                    RETURNING
                        id, username, status, role, scopes, preferences,
                        totp_enabled, session_refresh_ttl_secs,
                        created_at::text AS created_at,
                        disabled_at::text AS disabled_at,
                        deleted_at::text AS deleted_at
                    "#,
                )
                .bind(actor.operator.id)
                .bind(serde_json::json!(preferences))
                .fetch_optional(&mut *tx)
                .await?;
                let Some(row) = row else {
                    anyhow::bail!("operator not found");
                };
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (
                        id, actor_id, action, target, command_hash, metadata
                    )
                    VALUES ($1, $2, $3, $4, NULL, $5)
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind(actor.operator.id)
                .bind("operator.preferences.updated")
                .bind(format!("operator:{}", actor.operator.id))
                .bind(metadata)
                .execute(&mut *tx)
                .await?;
                let operator = operator_view_from_row(&row)?;
                tx.commit().await?;
                Ok(operator)
            }
        }
    }

    pub(crate) async fn list_operator_auth_events(
        &self,
        query: &OperatorAuthEventQuery,
    ) -> Result<Vec<OperatorAuthEventView>> {
        let limit = i64::from(query.limit.unwrap_or(100)).clamp(1, 200);
        match self {
            Self::Postgres(pool) => {
                let operator_id = query.operator_id.map(|id| id.to_string());
                let rows = sqlx::query(
                    r#"
                    SELECT id, metadata, created_at::text AS created_at
                    FROM audit_logs
                    WHERE action IN (
                        'operator_auth.login_success',
                        'operator_auth.login_failure',
                        'operator_auth.login_throttled'
                    )
                      AND ($2::text IS NULL OR metadata->>'operator_id' = $2)
                      AND (
                          $3::text IS NULL
                          OR COALESCE(
                              metadata->>'operator_username',
                              metadata->>'attempted_username'
                          ) = $3
                      )
                      AND ($4::text IS NULL OR metadata->>'result' = $4)
                    ORDER BY created_at DESC
                    LIMIT $1
                    "#,
                )
                .bind(limit)
                .bind(operator_id)
                .bind(
                    query
                        .username
                        .as_deref()
                        .map(str::trim)
                        .filter(|value| !value.is_empty()),
                )
                .bind(
                    query
                        .result
                        .as_deref()
                        .map(str::trim)
                        .filter(|value| !value.is_empty()),
                )
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(|row| operator_auth_event_from_row(&row))
                    .collect()
            }
        }
    }

    pub(crate) async fn list_operator_sessions(
        &self,
        limit: i64,
        current_session_id: Uuid,
    ) -> Result<Vec<OperatorSessionView>> {
        let limit = limit.clamp(1, 200);
        match self {
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        s.id,
                        s.operator_id,
                        o.username AS operator_username,
                        o.role AS operator_role,
                        s.created_at::text AS created_at,
                        s.expires_at::text AS expires_at,
                        s.refresh_expires_at::text AS refresh_expires_at,
                        s.revoked_at::text AS revoked_at
                    FROM operator_sessions s
                    JOIN operators o ON o.id = s.operator_id
                    ORDER BY s.created_at DESC
                    LIMIT $1
                    "#,
                )
                .bind(limit)
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(|row| {
                        let session_id = row.try_get("id")?;
                        let revoked_at: Option<String> = row.try_get("revoked_at")?;
                        Ok(OperatorSessionView {
                            id: session_id,
                            operator_id: row.try_get("operator_id")?,
                            operator_username: row.try_get("operator_username")?,
                            operator_role: row.try_get("operator_role")?,
                            current: session_id == current_session_id,
                            created_at: row.try_get("created_at")?,
                            expires_at: row.try_get("expires_at")?,
                            refresh_expires_at: row.try_get("refresh_expires_at")?,
                            revoked: revoked_at.is_some(),
                            revoked_at,
                        })
                    })
                    .collect()
            }
        }
    }

    pub(crate) async fn revoke_operator_sessions(
        &self,
        session_ids: &[Uuid],
        actor: &AuthContext,
    ) -> Result<Vec<AccessBatchMutationOutcome<OperatorSessionView>>> {
        let current_session_id = actor
            .audit_session_id()
            .context("operator session revoke requires an authenticated session")?;
        match self {
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let locked_rows = sqlx::query(
                    r#"
                    SELECT session.id
                    FROM operator_sessions AS session
                    WHERE session.id = ANY($1)
                    ORDER BY session.id
                    FOR UPDATE
                    "#,
                )
                .bind(session_ids)
                .fetch_all(&mut *tx)
                .await?;
                let locked_ids = locked_rows
                    .iter()
                    .map(|row| row.try_get::<Uuid, _>("id"))
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                let locked_id_set = locked_ids
                    .iter()
                    .copied()
                    .collect::<std::collections::HashSet<_>>();
                if locked_ids.is_empty() {
                    tx.rollback().await?;
                    return Ok(session_ids
                        .iter()
                        .map(|session_id| AccessBatchMutationOutcome::Rejected {
                            target_id: *session_id,
                            code: "operator_session_not_found",
                        })
                        .collect());
                }
                let rows = sqlx::query(
                    r#"
                    WITH revoked AS (
                        UPDATE operator_sessions
                        SET revoked_at = COALESCE(revoked_at, now())
                        WHERE id = ANY($1)
                        RETURNING
                            id,
                            operator_id,
                            created_at,
                            expires_at,
                            refresh_expires_at,
                            revoked_at
                    )
                    SELECT
                        revoked.id,
                        revoked.operator_id,
                        o.username AS operator_username,
                        o.role AS operator_role,
                        revoked.created_at::text AS created_at,
                        revoked.expires_at::text AS expires_at,
                        revoked.refresh_expires_at::text AS refresh_expires_at,
                        revoked.revoked_at::text AS revoked_at
                    FROM revoked
                    JOIN operators o ON o.id = revoked.operator_id
                    "#,
                )
                .bind(&locked_ids)
                .fetch_all(&mut *tx)
                .await?;
                let views = rows
                    .iter()
                    .map(|row| {
                        let session_id: Uuid = row.try_get("id")?;
                        let revoked_at: Option<String> = row.try_get("revoked_at")?;
                        Ok((
                            session_id,
                            OperatorSessionView {
                                id: session_id,
                                operator_id: row.try_get("operator_id")?,
                                operator_username: row.try_get("operator_username")?,
                                operator_role: row.try_get("operator_role")?,
                                current: session_id == current_session_id,
                                created_at: row.try_get("created_at")?,
                                expires_at: row.try_get("expires_at")?,
                                refresh_expires_at: row.try_get("refresh_expires_at")?,
                                revoked: revoked_at.is_some(),
                                revoked_at,
                            },
                        ))
                    })
                    .collect::<Result<HashMap<_, _>>>()?;
                let audit_ids = locked_ids
                    .iter()
                    .map(|_| Uuid::new_v4())
                    .collect::<Vec<_>>();
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (id, actor_id, action, target, command_hash, metadata)
                    SELECT
                        mutation.audit_id,
                        $3,
                        'operator_session.revoked',
                        'operator-session:' || mutation.session_id::text,
                        NULL,
                        jsonb_build_object(
                            'revoked_operator_session_id', mutation.session_id,
                            'operator_id', $3::uuid,
                            'operator_username', $4::text,
                            'operator_role', $5::text,
                            'operator_session_id', $6::uuid,
                            'result', 'succeeded',
                            'origin_kind', 'operator_request',
                            'component', 'operator-session-controller'
                        )
                    FROM unnest($1::uuid[], $2::uuid[])
                        AS mutation(audit_id, session_id)
                    "#,
                )
                .bind(&audit_ids)
                .bind(&locked_ids)
                .bind(actor.operator.id)
                .bind(&actor.operator.username)
                .bind(&actor.operator.role)
                .bind(actor.audit_session_id())
                .execute(&mut *tx)
                .await?;
                let outcomes = session_ids
                    .iter()
                    .map(|session_id| {
                        if !locked_id_set.contains(session_id) {
                            Ok(AccessBatchMutationOutcome::Rejected {
                                target_id: *session_id,
                                code: "operator_session_not_found",
                            })
                        } else {
                            Ok(AccessBatchMutationOutcome::Applied {
                                target_id: *session_id,
                                result: views.get(session_id).cloned().with_context(|| {
                                    format!(
                                        "operator session revoke omitted approved target {session_id}"
                                    )
                                })?,
                            })
                        }
                    })
                    .collect::<Result<Vec<_>>>()?;
                tx.commit().await?;
                Ok(outcomes)
            }
        }
    }

    /// Revokes the session row proven by an issued access token.
    ///
    /// This lookup intentionally ignores normal access expiry and prior revocation:
    /// an expired token must still be able to revoke its paired refresh token, and
    /// a retry after a lost response must remain successful without a second audit.
    pub(crate) async fn logout_operator_session(
        &self,
        access_token: &str,
        remote_ip: &str,
        user_agent: Option<&str>,
    ) -> Result<bool> {
        let access_hash = token_hash(access_token);
        match self {
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let row = sqlx::query(
                    r#"
                    SELECT
                        s.id,
                        s.operator_id,
                        o.username AS operator_username,
                        s.revoked_at IS NOT NULL AS revoked
                    FROM operator_sessions AS s
                    JOIN operators AS o ON o.id = s.operator_id
                    WHERE s.access_token_hash = $1
                    FOR UPDATE OF s
                    "#,
                )
                .bind(&access_hash)
                .fetch_optional(&mut *tx)
                .await?;
                let Some(row) = row else {
                    tx.rollback().await?;
                    return Ok(false);
                };
                let session_id: Uuid = row.try_get("id")?;
                let operator_id: Uuid = row.try_get("operator_id")?;
                let operator_username: String = row.try_get("operator_username")?;
                let revoked: bool = row.try_get("revoked")?;
                if !revoked {
                    sqlx::query("UPDATE operator_sessions SET revoked_at = now() WHERE id = $1")
                        .bind(session_id)
                        .execute(&mut *tx)
                        .await?;
                    sqlx::query(audit_insert_sql())
                        .bind(Uuid::new_v4())
                        .bind(operator_id)
                        .bind("operator_session.logged_out")
                        .bind(format!("operator-session:{session_id}"))
                        .bind(serde_json::json!({
                            "operator_id": operator_id,
                            "operator_username": operator_username,
                            "operator_session_id": session_id,
                            "remote_ip": remote_ip,
                            "user_agent": user_agent.unwrap_or(""),
                            "revocation_scope": "current_session",
                            "revoked_access_and_refresh": true,
                            "result": "succeeded",
                            "origin_kind": "operator_request",
                            "component": "operator-session-controller",
                        }))
                        .execute(&mut *tx)
                        .await?;
                }
                tx.commit().await?;
                Ok(true)
            }
        }
    }

    #[cfg(test)]
    pub(crate) async fn issue_session(&self, operator: OperatorView) -> Result<AuthResponse> {
        let session = PreparedOperatorSession::new(operator.session_refresh_ttl_secs);

        match self {
            Self::Postgres(pool) => {
                insert_operator_session(pool, operator.id, &session).await?;
            }
        }

        Ok(session.auth_response(operator))
    }

    async fn issue_password_login_session(
        &self,
        verified_operator: &OperatorRecord,
        context: SuccessfulOperatorAuthContext<'_>,
    ) -> Result<Option<AuthResponse>> {
        match self {
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let row = sqlx::query(
                    r#"
                    SELECT
                        id, username, status, role, scopes, preferences,
                        totp_enabled, session_refresh_ttl_secs,
                        created_at::text AS created_at,
                        disabled_at::text AS disabled_at,
                        deleted_at::text AS deleted_at
                    FROM operators
                    WHERE id = $1
                      AND status = 'active'
                      AND NOT totp_enabled
                      AND password_hash = $2
                    FOR UPDATE
                    "#,
                )
                .bind(verified_operator.id)
                .bind(&verified_operator.password_hash)
                .fetch_optional(&mut *tx)
                .await?;
                let Some(row) = row else {
                    tx.rollback().await?;
                    return Ok(None);
                };
                let operator = operator_view_from_row(&row)?;
                let session = PreparedOperatorSession::new(operator.session_refresh_ttl_secs);
                let event = PreparedOperatorAuthEvent::new(
                    Some((
                        operator.id,
                        operator.username.as_str(),
                        operator.role.as_str(),
                    )),
                    context.attempted_username,
                    "success",
                    None,
                    context.remote_ip,
                    context.user_agent,
                    Some(session.session_id),
                    context.cleared_previous_failures,
                );
                sqlx::query(
                    "DELETE FROM operator_auth_throttle WHERE scope_kind = 'username_ip' AND scope_key = $1",
                )
                .bind(context.username_key)
                .execute(&mut *tx)
                .await?;
                insert_operator_session_in_tx(&mut tx, operator.id, &session).await?;
                insert_operator_auth_event_in_tx(&mut tx, &event).await?;
                tx.commit().await?;
                Ok(Some(session.auth_response(operator)))
            }
        }
    }

    async fn issue_totp_login_session(
        &self,
        verified_operator: &OperatorRecord,
        matched_step: u64,
        success_context: Option<SuccessfulOperatorAuthContext<'_>>,
    ) -> Result<Option<AuthResponse>> {
        let session = PreparedOperatorSession::new(verified_operator.session_refresh_ttl_secs);
        match self {
            Self::Postgres(pool) => {
                let encrypted = verified_operator
                    .encrypted_totp_secret()
                    .context("enabled TOTP operator is missing secret material")?;
                let mut tx = pool.begin().await?;
                let row = sqlx::query(
                    r#"
                    UPDATE operators
                    SET totp_last_accepted_step = $2
                    WHERE id = $1
                      AND status = 'active'
                      AND totp_enabled
                      AND password_hash = $3
                      AND totp_secret_ciphertext_hex = $4
                      AND totp_secret_nonce_hex = $5
                      AND totp_secret_salt_hex = $6
                      AND (
                          totp_last_accepted_step IS NULL
                          OR totp_last_accepted_step < $2
                      )
                    RETURNING
                        id, username, status, role, scopes, preferences,
                        totp_enabled, session_refresh_ttl_secs,
                        created_at::text AS created_at,
                        disabled_at::text AS disabled_at,
                        deleted_at::text AS deleted_at
                    "#,
                )
                .bind(verified_operator.id)
                .bind(matched_step as i64)
                .bind(&verified_operator.password_hash)
                .bind(&encrypted.ciphertext_hex)
                .bind(&encrypted.nonce_hex)
                .bind(&encrypted.salt_hex)
                .fetch_optional(&mut *tx)
                .await?;
                let Some(row) = row else {
                    return Ok(None);
                };
                let operator = operator_view_from_row(&row)?;
                if let Some(context) = success_context {
                    sqlx::query(
                        "DELETE FROM operator_auth_throttle WHERE scope_kind = 'username_ip' AND scope_key = $1",
                    )
                    .bind(context.username_key)
                    .execute(&mut *tx)
                    .await?;
                }
                insert_operator_session_in_tx(&mut tx, operator.id, &session).await?;
                if let Some(context) = success_context {
                    let event = PreparedOperatorAuthEvent::new(
                        Some((
                            operator.id,
                            operator.username.as_str(),
                            operator.role.as_str(),
                        )),
                        context.attempted_username,
                        "success",
                        None,
                        context.remote_ip,
                        context.user_agent,
                        Some(session.session_id),
                        context.cleared_previous_failures,
                    );
                    insert_operator_auth_event_in_tx(&mut tx, &event).await?;
                }
                tx.commit().await?;
                Ok(Some(session.auth_response(operator)))
            }
        }
    }
}

fn normalize_session_refresh_ttl(value: u64) -> std::result::Result<u64, ApiError> {
    if (MIN_REFRESH_TOKEN_TTL_SECS..=MAX_REFRESH_TOKEN_TTL_SECS).contains(&value) {
        Ok(value)
    } else {
        Err(ApiError::bad_request("invalid_session_refresh_ttl_secs"))
    }
}

fn audit_insert_sql() -> &'static str {
    r#"
    INSERT INTO audit_logs (id, actor_id, action, target, command_hash, metadata)
    VALUES ($1, $2, $3, $4, NULL, $5)
    "#
}

fn operator_view_from_row(row: &sqlx::postgres::PgRow) -> Result<OperatorView> {
    Ok(OperatorView {
        id: row.try_get("id")?,
        username: row.try_get("username")?,
        status: row.try_get("status")?,
        role: row.try_get("role")?,
        scopes: parse_scopes(row.try_get("scopes")?),
        preferences: parse_operator_preferences(row.try_get("preferences")?),
        totp_enabled: row.try_get("totp_enabled")?,
        session_refresh_ttl_secs: row
            .try_get::<i64, _>("session_refresh_ttl_secs")?
            .try_into()
            .unwrap_or(DEFAULT_REFRESH_TOKEN_TTL_SECS),
        created_at: row.try_get("created_at")?,
        disabled_at: row.try_get("disabled_at")?,
        deleted_at: row.try_get("deleted_at")?,
    })
}

pub(crate) fn postgres_totp_step(row: &sqlx::postgres::PgRow) -> Result<Option<u64>> {
    row.try_get::<Option<i64>, _>("totp_last_accepted_step")?
        .map(u64::try_from)
        .transpose()
        .map_err(Into::into)
}

fn operator_auth_event_from_row(row: &sqlx::postgres::PgRow) -> Result<OperatorAuthEventView> {
    let metadata: serde_json::Value = row.try_get("metadata")?;
    Ok(OperatorAuthEventView {
        id: row.try_get("id")?,
        operator_id: json_uuid(&metadata, "operator_id")?,
        username: json_string(&metadata, "operator_username")
            .or_else(|| json_string(&metadata, "attempted_username"))
            .context("operator auth audit missing canonical username")?,
        result: json_string(&metadata, "result")
            .context("operator auth audit missing canonical result")?,
        reason: json_string(&metadata, "reason"),
        remote_ip: json_string(&metadata, "remote_ip"),
        user_agent: json_string(&metadata, "user_agent"),
        session_id: json_uuid(&metadata, "operator_session_id")?,
        created_at: row.try_get("created_at")?,
    })
}

fn json_string(value: &serde_json::Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn json_uuid(value: &serde_json::Value, key: &str) -> Result<Option<Uuid>> {
    match value.get(key) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(raw)) => {
            let parsed = Uuid::parse_str(raw.trim())
                .with_context(|| format!("operator auth audit has invalid {key}"))?;
            Ok(Some(parsed))
        }
        Some(_) => anyhow::bail!("operator auth audit {key} must be a UUID string"),
    }
}

struct PreparedOperatorAuthEvent {
    id: Uuid,
    actor_id: Option<Uuid>,
    action: &'static str,
    target: String,
    metadata: serde_json::Value,
}

impl PreparedOperatorAuthEvent {
    #[allow(clippy::too_many_arguments)]
    fn new(
        operator: Option<(Uuid, &str, &str)>,
        attempted_username: &str,
        result: &str,
        reason: Option<&str>,
        remote_ip: &str,
        user_agent: Option<&str>,
        session_id: Option<Uuid>,
        cleared_previous_failures: bool,
    ) -> Self {
        let action = match result {
            "success" => "operator_auth.login_success",
            "throttled" => "operator_auth.login_throttled",
            _ => "operator_auth.login_failure",
        };
        let username = attempted_username.trim();
        let normalized_username = if username.is_empty() {
            "<empty>".to_string()
        } else {
            username.to_string()
        };
        let authenticated_operator = if result == "success" { operator } else { None };
        let mut metadata = serde_json::json!({
            "attempted_username": normalized_username,
            "result": result,
            "reason": reason,
            "remote_ip": remote_ip,
            "user_agent": user_agent.unwrap_or(""),
            "cleared_previous_failures": cleared_previous_failures,
            "origin_kind": "authentication",
            "component": "operator-auth",
        });
        if let Some((operator_id, operator_username, operator_role)) = authenticated_operator {
            metadata["operator_id"] = serde_json::json!(operator_id);
            metadata["operator_username"] = serde_json::json!(operator_username);
            metadata["operator_role"] = serde_json::json!(operator_role);
            metadata["operator_session_id"] = serde_json::json!(session_id);
        }
        Self {
            id: Uuid::new_v4(),
            actor_id: authenticated_operator.map(|(operator_id, _, _)| operator_id),
            action,
            target: authenticated_operator
                .map(|(operator_id, _, _)| format!("operator:{operator_id}"))
                .unwrap_or_else(|| format!("operator-login:{normalized_username}")),
            metadata,
        }
    }
}

struct PreparedOperatorSession {
    access_token: String,
    refresh_token: String,
    session_id: Uuid,
    expires_unix: u64,
    refresh_expires_unix: u64,
    access_hash: String,
    refresh_hash: String,
}

impl PreparedOperatorSession {
    fn new(refresh_ttl_secs: u64) -> Self {
        let access_token = generate_token();
        let refresh_token = generate_token();
        let session_id = Uuid::new_v4();
        let created_unix = unix_now();
        let expires_unix = created_unix.saturating_add(ACCESS_TOKEN_TTL_SECS);
        let refresh_ttl_secs =
            refresh_ttl_secs.clamp(MIN_REFRESH_TOKEN_TTL_SECS, MAX_REFRESH_TOKEN_TTL_SECS);
        let refresh_expires_unix = created_unix.saturating_add(refresh_ttl_secs);
        let access_hash = token_hash(&access_token);
        let refresh_hash = token_hash(&refresh_token);

        Self {
            access_token,
            refresh_token,
            session_id,
            expires_unix,
            refresh_expires_unix,
            access_hash,
            refresh_hash,
        }
    }

    fn auth_response(self, operator: OperatorView) -> AuthResponse {
        AuthResponse {
            token_type: "Bearer",
            access_token: self.access_token,
            refresh_token: self.refresh_token,
            session_id: self.session_id,
            expires_in_secs: ACCESS_TOKEN_TTL_SECS,
            refresh_expires_in_secs: operator.session_refresh_ttl_secs,
            operator,
        }
    }
}

async fn insert_operator_auth_event(
    pool: &sqlx::PgPool,
    event: &PreparedOperatorAuthEvent,
) -> Result<()> {
    sqlx::query(operator_auth_event_insert_sql())
        .bind(event.id)
        .bind(event.actor_id)
        .bind(event.action)
        .bind(&event.target)
        .bind(&event.metadata)
        .execute(pool)
        .await?;
    Ok(())
}

async fn insert_operator_auth_event_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    event: &PreparedOperatorAuthEvent,
) -> Result<()> {
    sqlx::query(operator_auth_event_insert_sql())
        .bind(event.id)
        .bind(event.actor_id)
        .bind(event.action)
        .bind(&event.target)
        .bind(&event.metadata)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

fn operator_auth_event_insert_sql() -> &'static str {
    r#"
    INSERT INTO audit_logs (
        id, actor_id, action, target, command_hash, metadata
    )
    VALUES ($1, $2, $3, $4, NULL, $5)
    "#
}

#[cfg(test)]
async fn insert_operator_session(
    pool: &sqlx::PgPool,
    operator_id: Uuid,
    session: &PreparedOperatorSession,
) -> Result<()> {
    sqlx::query(operator_session_insert_sql())
        .bind(session.session_id)
        .bind(operator_id)
        .bind(&session.access_hash)
        .bind(&session.refresh_hash)
        .bind(session.expires_unix as f64)
        .bind(session.refresh_expires_unix as f64)
        .execute(pool)
        .await?;
    Ok(())
}

async fn insert_operator_session_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    operator_id: Uuid,
    session: &PreparedOperatorSession,
) -> Result<()> {
    sqlx::query(operator_session_insert_sql())
        .bind(session.session_id)
        .bind(operator_id)
        .bind(&session.access_hash)
        .bind(&session.refresh_hash)
        .bind(session.expires_unix as f64)
        .bind(session.refresh_expires_unix as f64)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

fn operator_session_insert_sql() -> &'static str {
    r#"
    INSERT INTO operator_sessions (
        id, operator_id, access_token_hash, refresh_token_hash,
        expires_at, refresh_expires_at
    )
    VALUES (
        $1, $2, $3, $4,
        to_timestamp($5::double precision),
        to_timestamp($6::double precision)
    )
    "#
}

fn normalize_auth_throttle_username(username: &str) -> String {
    let normalized = username.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        "<empty>".to_string()
    } else {
        normalized
    }
}

fn normalize_auth_throttle_identity(username: &str, remote_ip: &str) -> String {
    let username = normalize_auth_throttle_username(username);
    let remote_ip = normalize_auth_throttle_ip(remote_ip);
    format!("{}:{username}|{remote_ip}", username.len())
}

fn normalize_auth_throttle_ip(remote_ip: &str) -> String {
    let normalized = remote_ip.trim();
    if normalized.is_empty() {
        "<unknown>".to_string()
    } else {
        normalized.to_string()
    }
}

async fn record_postgres_throttle_failure(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    scope_kind: &'static str,
    scope_key: &str,
    attempt_limit: i64,
    window_secs: u64,
    lockout_secs: u64,
    reason: &str,
) -> Result<Option<AuthThrottleLockout>> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("operator_auth_throttle:{scope_kind}:{scope_key}"))
        .execute(&mut **tx)
        .await?;

    let existing = sqlx::query(
        r#"
        SELECT failed_attempts,
               window_started_at + make_interval(secs => $3::double precision) <= now()
                   AS window_expired,
               locked_until IS NOT NULL AND locked_until > now() AS was_locked
        FROM operator_auth_throttle
        WHERE scope_kind = $1
          AND scope_key = $2
        FOR UPDATE
        "#,
    )
    .bind(scope_kind)
    .bind(scope_key)
    .bind(window_secs as f64)
    .fetch_optional(&mut **tx)
    .await?;

    let (new_count, window_expired, was_locked) = if let Some(row) = existing {
        let window_expired: bool = row.try_get("window_expired")?;
        let failed_attempts: i64 = row.try_get("failed_attempts")?;
        let was_locked: bool = row.try_get("was_locked")?;
        (
            if window_expired {
                1
            } else {
                failed_attempts.saturating_add(1)
            },
            window_expired,
            was_locked,
        )
    } else {
        (1, true, false)
    };
    let lockout_created = !was_locked && new_count >= attempt_limit;

    sqlx::query(
        r#"
        INSERT INTO operator_auth_throttle (
            scope_kind,
            scope_key,
            failed_attempts,
            window_started_at,
            locked_until,
            last_failed_at,
            last_failure_reason,
            created_at,
            updated_at
        )
        VALUES (
            $1,
            $2,
            $3,
            now(),
            CASE WHEN $4 THEN now() + make_interval(secs => $5::double precision) ELSE NULL END,
            now(),
            $6,
            now(),
            now()
        )
        ON CONFLICT (scope_kind, scope_key) DO UPDATE
        SET failed_attempts = $3,
            window_started_at = CASE
                WHEN $7 THEN now()
                ELSE operator_auth_throttle.window_started_at
            END,
            locked_until = CASE
                WHEN $4 THEN now() + make_interval(secs => $5::double precision)
                WHEN $7 THEN NULL
                ELSE operator_auth_throttle.locked_until
            END,
            last_failed_at = now(),
            last_failure_reason = $6,
            updated_at = now()
        "#,
    )
    .bind(scope_kind)
    .bind(scope_key)
    .bind(new_count)
    .bind(new_count >= attempt_limit)
    .bind(lockout_secs as f64)
    .bind(reason)
    .bind(window_expired)
    .execute(&mut **tx)
    .await?;

    Ok(lockout_created.then(|| AuthThrottleLockout {
        scope_kind,
        scope_key: scope_key.to_string(),
        failed_attempts: new_count,
    }))
}

async fn insert_postgres_auth_lockout_audit(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    lockout: &AuthThrottleLockout,
    reason: &str,
    remote_ip: &str,
) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO audit_logs (
            id, actor_id, action, target, command_hash, metadata
        )
        VALUES ($1, NULL, $2, $3, NULL, $4)
        "#,
    )
    .bind(Uuid::new_v4())
    .bind("operator_auth.lockout_created")
    .bind("auth:login")
    .bind(auth_lockout_metadata(lockout, reason, remote_ip))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn auth_lockout_metadata(
    lockout: &AuthThrottleLockout,
    reason: &str,
    remote_ip: &str,
) -> serde_json::Value {
    serde_json::json!({
        "origin_kind": "authentication",
        "component": "operator-auth-throttle",
        "result": "locked",
        "remote_ip": remote_ip,
        "scope_kind": lockout.scope_kind,
        "scope_key": lockout.scope_key,
        "failed_attempts": lockout.failed_attempts,
        "last_failure_reason": reason,
    })
}

async fn ensure_postgres_active_admin_remains(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    _operator_id: Uuid,
    current_status: String,
    current_role: String,
    next_role: Option<&str>,
    next_status: Option<&str>,
) -> Result<()> {
    let row = sqlx::query(
        "SELECT count(*) AS count FROM operators WHERE status = 'active' AND role = 'admin'",
    )
    .fetch_one(&mut **tx)
    .await?;
    let active_admin_count: i64 = row.try_get("count")?;
    let will_remain_active_admin = is_active_admin(
        next_status.unwrap_or(current_status.as_str()),
        next_role.unwrap_or(current_role.as_str()),
    );
    if is_active_admin(&current_status, &current_role)
        && !will_remain_active_admin
        && active_admin_count <= 1
    {
        anyhow::bail!("last_active_admin_required");
    }
    Ok(())
}

async fn lock_postgres_active_admin_invariant(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<()> {
    // Only role/status transitions can reduce the active-admin set. Serialize
    // that exact invariant without blocking unrelated operator credential,
    // preference, authentication, or TOTP writes.
    sqlx::query(
        "SELECT pg_advisory_xact_lock(hashtextextended('vpsman:operator-active-admin-invariant', 0))",
    )
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn is_active_admin(status: &str, role: &str) -> bool {
    status == "active" && role == "admin"
}

pub(crate) fn parse_scopes(value: serde_json::Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|scope| scope.as_str().map(ToOwned::to_owned))
        .collect()
}

pub(crate) fn parse_operator_preferences(value: serde_json::Value) -> OperatorPreferences {
    serde_json::from_value::<OperatorPreferences>(value)
        .unwrap_or_default()
        .normalized()
}
