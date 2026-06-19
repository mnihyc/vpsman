use anyhow::Result;
use sqlx::Row;
use uuid::Uuid;

use crate::error::ApiError;
use crate::model::*;
use crate::repository::{MemoryState, OperatorAuthThrottleRecord, Repository};
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
            Self::Memory(memory) => Ok(memory.operators.read().await.len() as i64),
            Self::Postgres(pool) => {
                let row = sqlx::query("SELECT count(*) AS count FROM operators")
                    .fetch_one(pool)
                    .await?;
                Ok(row.try_get("count")?)
            }
        }
    }

    pub(crate) async fn bootstrap_operator(
        &self,
        request: &BootstrapOperatorRequest,
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
            session_refresh_ttl_secs: DEFAULT_REFRESH_TOKEN_TTL_SECS,
            created_at: now,
            disabled_at: None,
            deleted_at: None,
        };
        match self {
            Self::Memory(memory) => {
                let mut operators = memory.operators.write().await;
                if !operators.is_empty() {
                    anyhow::bail!("operator_already_bootstrapped");
                }
                operators.push(operator.clone());
                drop(operators);
                self.issue_session(operator.view()).await
            }
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
                let session = PreparedOperatorSession::new(operator.session_refresh_ttl_secs);
                insert_operator_session_in_tx(&mut tx, operator.id, &session).await?;
                tx.commit().await?;
                Ok(session.auth_response(operator.view()))
            }
        }
    }

    #[cfg(test)]
    pub(crate) async fn login_operator(
        &self,
        request: &LoginRequest,
    ) -> Result<Option<AuthResponse>> {
        let Some(operator) = self.operator_by_username(&request.username).await? else {
            return Ok(None);
        };
        if operator.status != "active" {
            return Ok(None);
        }
        if !verify_operator_password(&request.password, &operator.password_hash)? {
            return Ok(None);
        }
        if operator.totp_enabled {
            let Some(code) = request.totp_code.as_deref() else {
                return Ok(None);
            };
            let Some(secret) = operator.encrypted_totp_secret() else {
                return Ok(None);
            };
            let secret = match crate::auth_totp::decrypt_totp_secret(&request.password, &secret) {
                Ok(secret) => secret,
                Err(_) => return Ok(None),
            };
            if !crate::auth_totp::verify_totp_code(&secret, code, unix_now()) {
                return Ok(None);
            }
        }
        Ok(Some(self.issue_session(operator.view()).await?))
    }

    pub(crate) async fn login_operator_with_throttle(
        &self,
        request: &LoginRequest,
        remote_ip: &str,
        user_agent: Option<&str>,
        throttle: &OperatorAuthThrottleConfig,
    ) -> Result<OperatorLoginAttempt> {
        let username_key = normalize_auth_throttle_username(&request.username);
        let ip_key = normalize_auth_throttle_ip(remote_ip);
        if self
            .operator_auth_throttle_locked(&username_key, &ip_key)
            .await?
        {
            self.record_operator_auth_event(
                None,
                request.username.trim(),
                "throttled",
                Some("operator_login_throttled"),
                &ip_key,
                user_agent,
                None,
            )
            .await?;
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
        if operator.totp_enabled {
            let Some(code) = request.totp_code.as_deref() else {
                self.record_operator_auth_event(
                    Some(&operator),
                    request.username.trim(),
                    "failure",
                    Some(OperatorLoginFailureReason::MissingTotp.as_str()),
                    &ip_key,
                    user_agent,
                    None,
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
            if !crate::auth_totp::verify_totp_code(&secret, code, unix_now()) {
                self.record_operator_auth_event(
                    Some(&operator),
                    request.username.trim(),
                    "failure",
                    Some(OperatorLoginFailureReason::BadTotp.as_str()),
                    &ip_key,
                    user_agent,
                    None,
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
            }
        }
        let previous_failures = self
            .operator_auth_previous_failures(&username_key, throttle)
            .await?;
        self.clear_operator_auth_success(&username_key).await?;
        if previous_failures {
            self.record_operator_auth_success_after_failures(&operator, &username_key, &ip_key)
                .await?;
        }
        let response = self.issue_session(operator.view()).await?;
        self.record_operator_auth_event(
            Some(&operator),
            request.username.trim(),
            "success",
            None,
            &ip_key,
            user_agent,
            None,
        )
        .await?;
        Ok(OperatorLoginAttempt::Authenticated(Box::new(response)))
    }

    async fn operator_auth_throttle_locked(
        &self,
        username_key: &str,
        ip_key: &str,
    ) -> Result<bool> {
        match self {
            Self::Memory(memory) => {
                let now = unix_now();
                let throttle = memory.operator_auth_throttle.read().await;
                Ok(
                    throttle_bucket_locked(&throttle, "username", username_key, now)
                        || throttle_bucket_locked(&throttle, "ip", ip_key, now),
                )
            }
            Self::Postgres(pool) => {
                let row = sqlx::query(
                    r#"
                    SELECT EXISTS (
                        SELECT 1
                        FROM operator_auth_throttle
                        WHERE (
                            (scope_kind = 'username' AND scope_key = $1)
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
            Self::Memory(memory) => {
                let now = unix_now();
                let mut buckets = memory.operator_auth_throttle.write().await;
                let mut lockouts = Vec::new();
                if let Some(lockout) = record_memory_throttle_failure(
                    &mut buckets,
                    "username",
                    username_key,
                    throttle.username_failed_attempt_limit,
                    throttle.failed_attempt_window_secs,
                    throttle.lockout_secs,
                    reason.as_str(),
                    now,
                ) {
                    lockouts.push(lockout);
                }
                if let Some(lockout) = record_memory_throttle_failure(
                    &mut buckets,
                    "ip",
                    ip_key,
                    throttle.ip_failed_attempt_limit,
                    throttle.failed_attempt_window_secs,
                    throttle.lockout_secs,
                    reason.as_str(),
                    now,
                ) {
                    lockouts.push(lockout);
                }
                drop(buckets);
                for lockout in lockouts {
                    record_memory_auth_lockout_audit(memory, &lockout, reason.as_str()).await;
                }
                Ok(())
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let mut lockouts = Vec::new();
                if let Some(lockout) = record_postgres_throttle_failure(
                    &mut tx,
                    "username",
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
                    insert_postgres_auth_lockout_audit(&mut tx, lockout, reason.as_str()).await?;
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
            Self::Memory(memory) => {
                let now = unix_now();
                let buckets = memory.operator_auth_throttle.read().await;
                Ok(throttle_bucket_has_recent_failures(
                    &buckets,
                    "username",
                    username_key,
                    now,
                    throttle.failed_attempt_window_secs,
                ))
            }
            Self::Postgres(pool) => {
                let row = sqlx::query(
                    r#"
                    SELECT EXISTS (
                        SELECT 1
                        FROM operator_auth_throttle
                        WHERE (
                            (scope_kind = 'username' AND scope_key = $1)
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
            Self::Memory(memory) => {
                let mut buckets = memory.operator_auth_throttle.write().await;
                buckets.remove(&("username".to_string(), username_key.to_string()));
                Ok(())
            }
            Self::Postgres(pool) => {
                sqlx::query(
                    "DELETE FROM operator_auth_throttle WHERE scope_kind = 'username' AND scope_key = $1",
                )
                .bind(username_key)
                .execute(pool)
                .await?;
                Ok(())
            }
        }
    }

    async fn record_operator_auth_success_after_failures(
        &self,
        operator: &OperatorRecord,
        username_key: &str,
        ip_key: &str,
    ) -> Result<()> {
        let metadata = serde_json::json!({
            "operator_id": operator.id,
            "username": operator.username,
            "username_key": username_key,
            "ip": ip_key,
            "cleared_scope_kinds": ["username"],
        });
        match self {
            Self::Memory(memory) => {
                memory.audits.write().await.push(AuditLogView {
                    id: Uuid::new_v4(),
                    actor_id: Some(operator.id),
                    action: "operator_auth.login_after_failures".to_string(),
                    target: format!("operator:{}", operator.id),
                    command_hash: None,
                    metadata,
                    created_at: unix_now().to_string(),
                });
                Ok(())
            }
            Self::Postgres(pool) => {
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (
                        id, actor_id, action, target, command_hash, metadata
                    )
                    VALUES ($1, $2, $3, $4, NULL, $5)
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind(operator.id)
                .bind("operator_auth.login_after_failures")
                .bind(format!("operator:{}", operator.id))
                .bind(metadata)
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
    ) -> Result<()> {
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
        let metadata = serde_json::json!({
            "operator_id": operator.map(|operator| operator.id),
            "username": normalized_username,
            "result": result,
            "reason": reason,
            "remote_ip": remote_ip,
            "user_agent": user_agent.unwrap_or(""),
            "session_id": session_id,
        });
        match self {
            Self::Memory(memory) => {
                memory.audits.write().await.push(AuditLogView {
                    id: Uuid::new_v4(),
                    actor_id: operator.map(|operator| operator.id),
                    action: action.to_string(),
                    target: operator
                        .map(|operator| format!("operator:{}", operator.id))
                        .unwrap_or_else(|| format!("operator-login:{normalized_username}")),
                    command_hash: None,
                    metadata,
                    created_at: unix_now().to_string(),
                });
            }
            Self::Postgres(pool) => {
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (
                        id, actor_id, action, target, command_hash, metadata
                    )
                    VALUES ($1, $2, $3, $4, NULL, $5)
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind(operator.map(|operator| operator.id))
                .bind(action)
                .bind(
                    operator
                        .map(|operator| format!("operator:{}", operator.id))
                        .unwrap_or_else(|| format!("operator-login:{normalized_username}")),
                )
                .bind(metadata)
                .execute(pool)
                .await?;
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
            Self::Memory(memory) => {
                let now = unix_now();
                let mut sessions = memory.sessions.write().await;
                let Some(session) = sessions.iter_mut().find(|session| {
                    session.refresh_token_hash == refresh_hash
                        && !session.revoked
                        && session.refresh_expires_unix > now
                }) else {
                    return Ok(None);
                };
                session.revoked = true;
                let operator_id = session.operator_id;
                drop(sessions);
                let operators = memory.operators.read().await;
                let Some(operator) = operators.iter().find(|operator| operator.id == operator_id)
                else {
                    return Ok(None);
                };
                if operator.status != "active" {
                    return Ok(None);
                }
                self.issue_session(operator.view()).await.map(Some)
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let row = sqlx::query(
                    r#"
                    WITH revoked AS (
                        UPDATE operator_sessions
                        SET revoked_at = now()
                        WHERE refresh_token_hash = $1
                          AND refresh_expires_at > now()
                          AND revoked_at IS NULL
                        RETURNING operator_id
                    )
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
                    FROM revoked
                    JOIN operators o ON o.id = revoked.operator_id
                    WHERE o.status = 'active'
                    "#,
                )
                .bind(&refresh_hash)
                .fetch_optional(&mut *tx)
                .await?;
                let Some(row) = row else {
                    return Ok(None);
                };
                tx.commit().await?;
                let operator = OperatorView {
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
                };
                self.issue_session(operator).await.map(Some)
            }
        }
    }

    pub(crate) async fn authenticate_access_token(
        &self,
        access_token: &str,
    ) -> Result<Option<AuthContext>> {
        let access_hash = token_hash(access_token);
        match self {
            Self::Memory(memory) => {
                let now = unix_now();
                let sessions = memory.sessions.read().await;
                let Some(session) = sessions.iter().find(|session| {
                    session.access_token_hash == access_hash
                        && !session.revoked
                        && session.expires_unix > now
                }) else {
                    return Ok(None);
                };
                let operator_id = session.operator_id;
                let session_id = session.session_id;
                drop(sessions);
                Ok(memory
                    .operators
                    .read()
                    .await
                    .iter()
                    .find(|operator| operator.id == operator_id)
                    .filter(|operator| operator.status == "active")
                    .map(|operator| AuthContext {
                        operator: operator.view(),
                        session_id,
                    }))
            }
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
                        session_id: row.try_get("session_id")?,
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

    pub(crate) async fn operator_by_id(&self, id: Uuid) -> Result<Option<OperatorRecord>> {
        match self {
            Self::Memory(memory) => Ok(memory
                .operators
                .read()
                .await
                .iter()
                .find(|operator| operator.id == id)
                .cloned()),
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
            Self::Memory(memory) => Ok(memory
                .operators
                .read()
                .await
                .iter()
                .find(|operator| operator.username == username)
                .cloned()),
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
            Self::Memory(memory) => Ok(memory
                .operators
                .read()
                .await
                .iter()
                .map(OperatorRecord::view)
                .collect()),
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
            session_refresh_ttl_secs,
            created_at: now,
            disabled_at: None,
            deleted_at: None,
        };
        let metadata = serde_json::json!({
            "operator_id": operator.id,
            "username": operator.username,
            "role": operator.role,
            "scopes": operator.scopes,
            "session_refresh_ttl_secs": operator.session_refresh_ttl_secs,
            "created_by_operator_id": actor.operator.id,
            "created_by_operator_username": actor.operator.username,
            "session_id": actor.session_id,
        });
        match self {
            Self::Memory(memory) => {
                if memory
                    .operators
                    .read()
                    .await
                    .iter()
                    .any(|existing| existing.username == operator.username)
                {
                    anyhow::bail!("operator username already exists");
                }
                memory.operators.write().await.push(operator.clone());
                memory.audits.write().await.push(AuditLogView {
                    id: Uuid::new_v4(),
                    actor_id: Some(actor.operator.id),
                    action: "operator.created".to_string(),
                    target: format!("operator:{}", operator.id),
                    command_hash: None,
                    metadata,
                    created_at: unix_now().to_string(),
                });
            }
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
            "operator_id": operator_id,
            "role": role,
            "scopes": scopes,
            "session_refresh_ttl_secs": session_refresh_ttl_secs,
            "updated_by_operator_id": actor.operator.id,
            "updated_by_operator_username": actor.operator.username,
            "session_id": actor.session_id,
        });
        match self {
            Self::Memory(memory) => {
                let mut operators = memory.operators.write().await;
                let Some(index) = operators.iter().position(|operator| {
                    operator.id == operator_id && operator.status != "deleted"
                }) else {
                    return Ok(None);
                };
                ensure_memory_active_admin_remains(&operators, operator_id, Some(&role), None)?;
                let operator = operators
                    .get_mut(index)
                    .expect("operator index checked before mutation");
                operator.role = role;
                operator.scopes = scopes;
                operator.session_refresh_ttl_secs = session_refresh_ttl_secs;
                let view = operator.view();
                drop(operators);
                memory.audits.write().await.push(AuditLogView {
                    id: Uuid::new_v4(),
                    actor_id: Some(actor.operator.id),
                    action: "operator.updated".to_string(),
                    target: format!("operator:{operator_id}"),
                    command_hash: None,
                    metadata,
                    created_at: unix_now().to_string(),
                });
                Ok(Some(view))
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                sqlx::query("LOCK TABLE operators IN SHARE ROW EXCLUSIVE MODE")
                    .execute(&mut *tx)
                    .await?;
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
                tx.commit().await?;
                Ok(Some(operator_view_from_row(&row)?))
            }
        }
    }

    pub(crate) async fn set_operator_status(
        &self,
        operator_id: Uuid,
        status: &str,
        actor: &AuthContext,
    ) -> Result<Option<OperatorView>> {
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
        let metadata = serde_json::json!({
            "operator_id": operator_id,
            "status": status,
            "updated_by_operator_id": actor.operator.id,
            "updated_by_operator_username": actor.operator.username,
            "session_id": actor.session_id,
        });
        match self {
            Self::Memory(memory) => {
                let now = unix_now().to_string();
                let mut operators = memory.operators.write().await;
                let Some(index) = operators.iter().position(|operator| {
                    operator.id == operator_id
                        && operator.status != "deleted"
                        && (status != "active" || operator.status == "disabled")
                }) else {
                    return Ok(None);
                };
                ensure_memory_active_admin_remains(&operators, operator_id, None, Some(status))?;
                let operator = operators
                    .get_mut(index)
                    .expect("operator index checked before mutation");
                operator.status = status.to_string();
                if status == "active" {
                    operator.disabled_at = None;
                } else if status == "disabled" {
                    operator.disabled_at = Some(now.clone());
                } else {
                    operator.disabled_at = Some(now.clone());
                    operator.deleted_at = Some(now.clone());
                }
                let view = operator.view();
                drop(operators);
                if status != "active" {
                    revoke_memory_operator_sessions(memory, operator_id).await;
                }
                memory.audits.write().await.push(AuditLogView {
                    id: Uuid::new_v4(),
                    actor_id: Some(actor.operator.id),
                    action: action.to_string(),
                    target: format!("operator:{operator_id}"),
                    command_hash: None,
                    metadata,
                    created_at: now,
                });
                Ok(Some(view))
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                sqlx::query("LOCK TABLE operators IN SHARE ROW EXCLUSIVE MODE")
                    .execute(&mut *tx)
                    .await?;
                let target = sqlx::query(
                    r#"
                    SELECT status, role
                    FROM operators
                    WHERE id = $1
                      AND status <> 'deleted'
                      AND ($2 <> 'active' OR status = 'disabled')
                    FOR UPDATE
                    "#,
                )
                .bind(operator_id)
                .bind(status)
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
                    None,
                    Some(status),
                )
                .await?;
                let row = sqlx::query(
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
                    WHERE id = $1
                      AND status <> 'deleted'
                      AND ($2 <> 'active' OR status = 'disabled')
                    RETURNING
                        id, username, status, role, scopes, preferences,
                        totp_enabled, session_refresh_ttl_secs,
                        created_at::text AS created_at,
                        disabled_at::text AS disabled_at,
                        deleted_at::text AS deleted_at
                    "#,
                )
                .bind(operator_id)
                .bind(status)
                .fetch_optional(&mut *tx)
                .await?;
                let Some(row) = row else {
                    return Ok(None);
                };
                if status != "active" {
                    sqlx::query(
                        "UPDATE operator_sessions SET revoked_at = COALESCE(revoked_at, now()) WHERE operator_id = $1",
                    )
                    .bind(operator_id)
                    .execute(&mut *tx)
                    .await?;
                }
                sqlx::query(audit_insert_sql())
                    .bind(Uuid::new_v4())
                    .bind(actor.operator.id)
                    .bind(action)
                    .bind(format!("operator:{operator_id}"))
                    .bind(metadata)
                    .execute(&mut *tx)
                    .await?;
                tx.commit().await?;
                Ok(Some(operator_view_from_row(&row)?))
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
            "operator_id": operator_id,
            "reset_by_operator_id": actor.operator.id,
            "reset_by_operator_username": actor.operator.username,
            "session_id": actor.session_id,
            "sessions_revoked": true,
            "totp_cleared": true,
        });
        match self {
            Self::Memory(memory) => {
                let mut operators = memory.operators.write().await;
                let Some(operator) = operators
                    .iter_mut()
                    .find(|operator| operator.id == operator_id && operator.status != "deleted")
                else {
                    return Ok(None);
                };
                operator.password_hash = password_hash;
                operator.totp_enabled = false;
                operator.totp_secret_ciphertext_hex = None;
                operator.totp_secret_nonce_hex = None;
                operator.totp_secret_salt_hex = None;
                let view = operator.view();
                drop(operators);
                revoke_memory_operator_sessions(memory, operator_id).await;
                memory.audits.write().await.push(AuditLogView {
                    id: Uuid::new_v4(),
                    actor_id: Some(actor.operator.id),
                    action: "operator.password_reset".to_string(),
                    target: format!("operator:{operator_id}"),
                    command_hash: None,
                    metadata,
                    created_at: unix_now().to_string(),
                });
                Ok(Some(view))
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let row = sqlx::query(
                    r#"
                    UPDATE operators
                    SET password_hash = $2,
                        totp_enabled = FALSE,
                        totp_secret_ciphertext_hex = NULL,
                        totp_secret_nonce_hex = NULL,
                        totp_secret_salt_hex = NULL
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
                tx.commit().await?;
                Ok(Some(operator_view_from_row(&row)?))
            }
        }
    }

    pub(crate) async fn clear_operator_totp(
        &self,
        operator_id: Uuid,
        actor: &AuthContext,
    ) -> Result<Option<OperatorView>> {
        let metadata = serde_json::json!({
            "operator_id": operator_id,
            "cleared_by_operator_id": actor.operator.id,
            "cleared_by_operator_username": actor.operator.username,
            "session_id": actor.session_id,
            "sessions_revoked": true,
        });
        match self {
            Self::Memory(memory) => {
                let mut operators = memory.operators.write().await;
                let Some(operator) = operators
                    .iter_mut()
                    .find(|operator| operator.id == operator_id && operator.status != "deleted")
                else {
                    return Ok(None);
                };
                operator.totp_enabled = false;
                operator.totp_secret_ciphertext_hex = None;
                operator.totp_secret_nonce_hex = None;
                operator.totp_secret_salt_hex = None;
                let view = operator.view();
                drop(operators);
                revoke_memory_operator_sessions(memory, operator_id).await;
                memory.audits.write().await.push(AuditLogView {
                    id: Uuid::new_v4(),
                    actor_id: Some(actor.operator.id),
                    action: "operator.totp_cleared".to_string(),
                    target: format!("operator:{operator_id}"),
                    command_hash: None,
                    metadata,
                    created_at: unix_now().to_string(),
                });
                Ok(Some(view))
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let row = sqlx::query(
                    r#"
                    UPDATE operators
                    SET totp_enabled = false,
                        totp_secret_ciphertext_hex = NULL,
                        totp_secret_nonce_hex = NULL,
                        totp_secret_salt_hex = NULL
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
                    .bind("operator.totp_cleared")
                    .bind(format!("operator:{operator_id}"))
                    .bind(metadata)
                    .execute(&mut *tx)
                    .await?;
                tx.commit().await?;
                Ok(Some(operator_view_from_row(&row)?))
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
            "session_id": actor.session_id,
            "preferences": preferences,
        });
        match self {
            Self::Memory(memory) => {
                let mut operators = memory.operators.write().await;
                let Some(operator) = operators
                    .iter_mut()
                    .find(|operator| operator.id == actor.operator.id)
                else {
                    anyhow::bail!("operator not found");
                };
                operator.preferences = preferences.clone();
                let view = operator.view();
                drop(operators);
                memory.audits.write().await.push(AuditLogView {
                    id: Uuid::new_v4(),
                    actor_id: Some(actor.operator.id),
                    action: "operator.preferences.updated".to_string(),
                    target: format!("operator:{}", actor.operator.id),
                    command_hash: None,
                    metadata,
                    created_at: unix_now().to_string(),
                });
                Ok(view)
            }
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
                tx.commit().await?;
                Ok(operator_view_from_row(&row)?)
            }
        }
    }

    pub(crate) async fn list_operator_auth_events(
        &self,
        query: &OperatorAuthEventQuery,
    ) -> Result<Vec<OperatorAuthEventView>> {
        let limit = i64::from(query.limit.unwrap_or(100)).clamp(1, 200);
        match self {
            Self::Memory(memory) => {
                let mut rows = memory
                    .audits
                    .read()
                    .await
                    .iter()
                    .filter(|audit| is_operator_auth_event_action(&audit.action))
                    .filter_map(|audit| operator_auth_event_from_audit(audit).ok())
                    .filter(|event| {
                        query
                            .operator_id
                            .is_none_or(|operator_id| event.operator_id == Some(operator_id))
                    })
                    .filter(|event| {
                        query
                            .username
                            .as_ref()
                            .is_none_or(|username| event.username == username.trim())
                    })
                    .filter(|event| {
                        query
                            .result
                            .as_ref()
                            .is_none_or(|result| event.result == result.trim())
                    })
                    .collect::<Vec<_>>();
                rows.sort_by(|left, right| right.created_at.cmp(&left.created_at));
                rows.truncate(limit as usize);
                Ok(rows)
            }
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
                      AND ($3::text IS NULL OR metadata->>'username' = $3)
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
            Self::Memory(memory) => {
                let operators = memory.operators.read().await.clone();
                let mut sessions = memory
                    .sessions
                    .read()
                    .await
                    .iter()
                    .filter_map(|session| {
                        let operator = operators
                            .iter()
                            .find(|operator| operator.id == session.operator_id)?;
                        Some(OperatorSessionView {
                            id: session.session_id,
                            operator_id: operator.id,
                            operator_username: operator.username.clone(),
                            operator_role: operator.role.clone(),
                            current: session.session_id == current_session_id,
                            created_at: session.created_unix.to_string(),
                            expires_at: session.expires_unix.to_string(),
                            refresh_expires_at: session.refresh_expires_unix.to_string(),
                            revoked: session.revoked,
                            revoked_at: None,
                        })
                    })
                    .collect::<Vec<_>>();
                sessions.sort_by(|left, right| right.created_at.cmp(&left.created_at));
                sessions.truncate(limit as usize);
                Ok(sessions)
            }
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

    pub(crate) async fn operator_session_by_id(
        &self,
        session_id: Uuid,
        current_session_id: Uuid,
    ) -> Result<Option<OperatorSessionView>> {
        match self {
            Self::Memory(memory) => {
                let operators = memory.operators.read().await.clone();
                let sessions = memory.sessions.read().await;
                let Some(session) = sessions
                    .iter()
                    .find(|session| session.session_id == session_id)
                else {
                    return Ok(None);
                };
                let Some(operator) = operators
                    .iter()
                    .find(|operator| operator.id == session.operator_id)
                else {
                    return Ok(None);
                };
                Ok(Some(OperatorSessionView {
                    id: session.session_id,
                    operator_id: operator.id,
                    operator_username: operator.username.clone(),
                    operator_role: operator.role.clone(),
                    current: session.session_id == current_session_id,
                    created_at: session.created_unix.to_string(),
                    expires_at: session.expires_unix.to_string(),
                    refresh_expires_at: session.refresh_expires_unix.to_string(),
                    revoked: session.revoked,
                    revoked_at: None,
                }))
            }
            Self::Postgres(pool) => {
                let row = sqlx::query(
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
                    WHERE s.id = $1
                    "#,
                )
                .bind(session_id)
                .fetch_optional(pool)
                .await?;
                let Some(row) = row else {
                    return Ok(None);
                };
                let revoked_at: Option<String> = row.try_get("revoked_at")?;
                Ok(Some(OperatorSessionView {
                    id: row.try_get("id")?,
                    operator_id: row.try_get("operator_id")?,
                    operator_username: row.try_get("operator_username")?,
                    operator_role: row.try_get("operator_role")?,
                    current: session_id == current_session_id,
                    created_at: row.try_get("created_at")?,
                    expires_at: row.try_get("expires_at")?,
                    refresh_expires_at: row.try_get("refresh_expires_at")?,
                    revoked: revoked_at.is_some(),
                    revoked_at,
                }))
            }
        }
    }

    pub(crate) async fn revoke_operator_session(
        &self,
        session_id: Uuid,
        actor: &AuthContext,
    ) -> Result<Option<OperatorSessionView>> {
        match self {
            Self::Memory(memory) => {
                let mut sessions = memory.sessions.write().await;
                let Some(session) = sessions
                    .iter_mut()
                    .find(|session| session.session_id == session_id)
                else {
                    return Ok(None);
                };
                session.revoked = true;
                drop(sessions);
                record_session_revoke_audit(memory, session_id, actor).await;
                Ok(self
                    .list_operator_sessions(200, actor.session_id)
                    .await?
                    .into_iter()
                    .find(|session| session.id == session_id))
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let row = sqlx::query(
                    r#"
                    UPDATE operator_sessions
                    SET revoked_at = COALESCE(revoked_at, now())
                    WHERE id = $1
                    RETURNING id
                    "#,
                )
                .bind(session_id)
                .fetch_optional(&mut *tx)
                .await?;
                let Some(_) = row else {
                    return Ok(None);
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
                .bind("operator_session.revoked")
                .bind(format!("operator-session:{session_id}"))
                .bind(serde_json::json!({
                    "session_id": session_id,
                    "revoked_by_operator_id": actor.operator.id,
                    "revoked_by_operator_username": actor.operator.username,
                    "revoked_by_session_id": actor.session_id,
                }))
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                Ok(self
                    .list_operator_sessions(200, actor.session_id)
                    .await?
                    .into_iter()
                    .find(|session| session.id == session_id))
            }
        }
    }

    pub(crate) async fn issue_session(&self, operator: OperatorView) -> Result<AuthResponse> {
        let session = PreparedOperatorSession::new(operator.session_refresh_ttl_secs);

        match self {
            Self::Memory(memory) => {
                memory.sessions.write().await.push(OperatorSessionRecord {
                    session_id: session.session_id,
                    access_token_hash: session.access_hash.clone(),
                    refresh_token_hash: session.refresh_hash.clone(),
                    operator_id: operator.id,
                    expires_unix: session.expires_unix,
                    refresh_expires_unix: session.refresh_expires_unix,
                    created_unix: session.created_unix,
                    revoked: false,
                });
            }
            Self::Postgres(pool) => {
                insert_operator_session(pool, operator.id, &session).await?;
            }
        }

        Ok(session.auth_response(operator))
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

async fn revoke_memory_operator_sessions(memory: &MemoryState, operator_id: Uuid) {
    for session in memory.sessions.write().await.iter_mut() {
        if session.operator_id == operator_id {
            session.revoked = true;
        }
    }
}

fn is_operator_auth_event_action(action: &str) -> bool {
    matches!(
        action,
        "operator_auth.login_success"
            | "operator_auth.login_failure"
            | "operator_auth.login_throttled"
    )
}

fn operator_auth_event_from_audit(audit: &AuditLogView) -> Result<OperatorAuthEventView> {
    if !is_operator_auth_event_action(&audit.action) {
        anyhow::bail!("not operator auth event");
    }
    Ok(OperatorAuthEventView {
        id: audit.id,
        operator_id: json_uuid(&audit.metadata, "operator_id"),
        username: json_string(&audit.metadata, "username").unwrap_or_default(),
        result: json_string(&audit.metadata, "result").unwrap_or_else(|| {
            if audit.action == "operator_auth.login_success" {
                "success".to_string()
            } else if audit.action == "operator_auth.login_throttled" {
                "throttled".to_string()
            } else {
                "failure".to_string()
            }
        }),
        reason: json_string(&audit.metadata, "reason"),
        remote_ip: json_string(&audit.metadata, "remote_ip"),
        user_agent: json_string(&audit.metadata, "user_agent"),
        session_id: json_uuid(&audit.metadata, "session_id"),
        created_at: audit.created_at.clone(),
    })
}

fn operator_auth_event_from_row(row: &sqlx::postgres::PgRow) -> Result<OperatorAuthEventView> {
    let metadata: serde_json::Value = row.try_get("metadata")?;
    Ok(OperatorAuthEventView {
        id: row.try_get("id")?,
        operator_id: json_uuid(&metadata, "operator_id"),
        username: json_string(&metadata, "username").unwrap_or_default(),
        result: json_string(&metadata, "result").unwrap_or_else(|| "failure".to_string()),
        reason: json_string(&metadata, "reason"),
        remote_ip: json_string(&metadata, "remote_ip"),
        user_agent: json_string(&metadata, "user_agent"),
        session_id: json_uuid(&metadata, "session_id"),
        created_at: row.try_get("created_at")?,
    })
}

fn json_string(value: &serde_json::Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .filter(|value| !value.is_empty())
}

fn json_uuid(value: &serde_json::Value, key: &str) -> Option<Uuid> {
    json_string(value, key).and_then(|value| Uuid::parse_str(&value).ok())
}

struct PreparedOperatorSession {
    access_token: String,
    refresh_token: String,
    session_id: Uuid,
    created_unix: u64,
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
            created_unix,
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
            expires_in_secs: ACCESS_TOKEN_TTL_SECS,
            refresh_expires_in_secs: operator.session_refresh_ttl_secs,
            operator,
        }
    }
}

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

fn normalize_auth_throttle_ip(remote_ip: &str) -> String {
    let normalized = remote_ip.trim();
    if normalized.is_empty() {
        "<unknown>".to_string()
    } else {
        normalized.to_string()
    }
}

fn throttle_bucket_locked(
    buckets: &std::collections::HashMap<(String, String), OperatorAuthThrottleRecord>,
    scope_kind: &str,
    scope_key: &str,
    now: u64,
) -> bool {
    buckets
        .get(&(scope_kind.to_string(), scope_key.to_string()))
        .and_then(|bucket| bucket.locked_until_unix)
        .is_some_and(|locked_until| locked_until > now)
}

fn throttle_bucket_has_recent_failures(
    buckets: &std::collections::HashMap<(String, String), OperatorAuthThrottleRecord>,
    scope_kind: &str,
    scope_key: &str,
    now: u64,
    window_secs: u64,
) -> bool {
    buckets
        .get(&(scope_kind.to_string(), scope_key.to_string()))
        .is_some_and(|bucket| {
            bucket.failed_attempts > 0
                && (now.saturating_sub(bucket.window_started_unix) < window_secs
                    || bucket
                        .locked_until_unix
                        .is_some_and(|locked_until| locked_until > now))
        })
}

fn record_memory_throttle_failure(
    buckets: &mut std::collections::HashMap<(String, String), OperatorAuthThrottleRecord>,
    scope_kind: &'static str,
    scope_key: &str,
    attempt_limit: i64,
    window_secs: u64,
    lockout_secs: u64,
    reason: &str,
    now: u64,
) -> Option<AuthThrottleLockout> {
    let key = (scope_kind.to_string(), scope_key.to_string());
    let bucket = buckets
        .entry(key)
        .or_insert_with(|| OperatorAuthThrottleRecord {
            window_started_unix: now,
            ..OperatorAuthThrottleRecord::default()
        });
    let was_locked = bucket
        .locked_until_unix
        .is_some_and(|locked_until| locked_until > now);
    if now.saturating_sub(bucket.window_started_unix) >= window_secs {
        bucket.failed_attempts = 0;
        bucket.window_started_unix = now;
        bucket.locked_until_unix = None;
    }
    bucket.failed_attempts = bucket.failed_attempts.saturating_add(1);
    bucket.last_failure_reason = Some(reason.to_string());
    if bucket.failed_attempts >= attempt_limit {
        bucket.locked_until_unix = Some(now.saturating_add(lockout_secs));
    }
    let is_locked = bucket
        .locked_until_unix
        .is_some_and(|locked_until| locked_until > now);
    (is_locked && !was_locked).then(|| AuthThrottleLockout {
        scope_kind,
        scope_key: scope_key.to_string(),
        failed_attempts: bucket.failed_attempts,
    })
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

async fn record_memory_auth_lockout_audit(
    memory: &crate::repository::MemoryState,
    lockout: &AuthThrottleLockout,
    reason: &str,
) {
    memory.audits.write().await.push(AuditLogView {
        id: Uuid::new_v4(),
        actor_id: None,
        action: "operator_auth.lockout_created".to_string(),
        target: format!("operator-auth:{}:{}", lockout.scope_kind, lockout.scope_key),
        command_hash: None,
        metadata: auth_lockout_metadata(lockout, reason),
        created_at: unix_now().to_string(),
    });
}

async fn insert_postgres_auth_lockout_audit(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    lockout: &AuthThrottleLockout,
    reason: &str,
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
    .bind(format!(
        "operator-auth:{}:{}",
        lockout.scope_kind, lockout.scope_key
    ))
    .bind(auth_lockout_metadata(lockout, reason))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn auth_lockout_metadata(lockout: &AuthThrottleLockout, reason: &str) -> serde_json::Value {
    serde_json::json!({
        "scope_kind": lockout.scope_kind,
        "scope_key": lockout.scope_key,
        "failed_attempts": lockout.failed_attempts,
        "last_failure_reason": reason,
    })
}

fn ensure_memory_active_admin_remains(
    operators: &[OperatorRecord],
    operator_id: Uuid,
    next_role: Option<&str>,
    next_status: Option<&str>,
) -> Result<()> {
    let Some(operator) = operators.iter().find(|operator| operator.id == operator_id) else {
        return Ok(());
    };
    let active_admin_count = operators
        .iter()
        .filter(|operator| is_active_admin(&operator.status, &operator.role))
        .count();
    let will_remain_active_admin = is_active_admin(
        next_status.unwrap_or(operator.status.as_str()),
        next_role.unwrap_or(operator.role.as_str()),
    );
    if is_active_admin(&operator.status, &operator.role)
        && !will_remain_active_admin
        && active_admin_count <= 1
    {
        anyhow::bail!("last_active_admin_required");
    }
    Ok(())
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

async fn record_session_revoke_audit(
    memory: &crate::repository::MemoryState,
    session_id: Uuid,
    actor: &AuthContext,
) {
    memory.audits.write().await.push(AuditLogView {
        id: Uuid::new_v4(),
        actor_id: Some(actor.operator.id),
        action: "operator_session.revoked".to_string(),
        target: format!("operator-session:{session_id}"),
        command_hash: None,
        metadata: serde_json::json!({
            "session_id": session_id,
            "revoked_by_operator_id": actor.operator.id,
            "revoked_by_operator_username": actor.operator.username,
            "revoked_by_session_id": actor.session_id,
        }),
        created_at: unix_now().to_string(),
    });
}
