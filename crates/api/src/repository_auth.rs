use anyhow::Result;
use sqlx::Row;
use uuid::Uuid;

use crate::model::*;
use crate::repository::Repository;
use crate::{
    generate_token, hash_operator_password, normalize_operator_scopes, token_hash, unix_now,
    verify_operator_password, ACCESS_TOKEN_TTL_SECS, REFRESH_TOKEN_TTL_SECS,
};

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
        let operator = OperatorRecord {
            id: Uuid::new_v4(),
            username: request.username.trim().to_string(),
            password_hash: hash_operator_password(&request.password)?,
            role: "admin".to_string(),
            scopes: normalize_operator_scopes("admin", &[])
                .map_err(|error| anyhow::anyhow!(error.code))?,
            totp_enabled: false,
            totp_secret_ciphertext_hex: None,
            totp_secret_nonce_hex: None,
            totp_secret_salt_hex: None,
        };
        match self {
            Self::Memory(memory) => {
                memory.operators.write().await.push(operator.clone());
                self.issue_session(operator.view()).await
            }
            Self::Postgres(pool) => {
                sqlx::query(
                    r#"
                    INSERT INTO operators (id, username, password_hash, role, scopes)
                    VALUES ($1, $2, $3, $4, $5)
                    "#,
                )
                .bind(operator.id)
                .bind(&operator.username)
                .bind(&operator.password_hash)
                .bind(&operator.role)
                .bind(serde_json::json!(operator.scopes))
                .execute(pool)
                .await?;
                self.issue_session(operator.view()).await
            }
        }
    }

    pub(crate) async fn login_operator(
        &self,
        request: &LoginRequest,
    ) -> Result<Option<AuthResponse>> {
        let Some(operator) = self.operator_by_username(&request.username).await? else {
            return Ok(None);
        };
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
                self.issue_session(operator.view()).await.map(Some)
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let row = sqlx::query(
                    r#"
                    SELECT o.id, o.username, o.role, o.scopes, o.totp_enabled
                    FROM operator_sessions s
                    JOIN operators o ON o.id = s.operator_id
                    WHERE s.refresh_token_hash = $1
                      AND s.refresh_expires_at > now()
                      AND s.revoked_at IS NULL
                    "#,
                )
                .bind(&refresh_hash)
                .fetch_optional(&mut *tx)
                .await?;
                let Some(row) = row else {
                    return Ok(None);
                };
                sqlx::query(
                    r#"
                    UPDATE operator_sessions
                    SET revoked_at = now()
                    WHERE refresh_token_hash = $1
                    "#,
                )
                .bind(&refresh_hash)
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                let operator = OperatorView {
                    id: row.try_get("id")?,
                    username: row.try_get("username")?,
                    role: row.try_get("role")?,
                    scopes: parse_scopes(row.try_get("scopes")?),
                    totp_enabled: row.try_get("totp_enabled")?,
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
                        o.role,
                        o.scopes,
                        o.totp_enabled
                    FROM operator_sessions s
                    JOIN operators o ON o.id = s.operator_id
                    WHERE s.access_token_hash = $1
                      AND s.expires_at > now()
                      AND s.revoked_at IS NULL
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
                            role: row.try_get("role")?,
                            scopes: parse_scopes(row.try_get("scopes")?),
                            totp_enabled: row.try_get("totp_enabled")?,
                        },
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
                        role,
                        scopes,
                        totp_enabled,
                        totp_secret_ciphertext_hex,
                        totp_secret_nonce_hex,
                        totp_secret_salt_hex
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
                        role: row.try_get("role")?,
                        scopes: parse_scopes(row.try_get("scopes")?),
                        totp_enabled: row.try_get("totp_enabled")?,
                        totp_secret_ciphertext_hex: row.try_get("totp_secret_ciphertext_hex")?,
                        totp_secret_nonce_hex: row.try_get("totp_secret_nonce_hex")?,
                        totp_secret_salt_hex: row.try_get("totp_secret_salt_hex")?,
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
                    SELECT id, username, role, scopes, totp_enabled
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
                            role: row.try_get("role")?,
                            scopes: parse_scopes(row.try_get("scopes")?),
                            totp_enabled: row.try_get("totp_enabled")?,
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
        let operator = OperatorRecord {
            id: Uuid::new_v4(),
            username,
            password_hash: hash_operator_password(&request.password)?,
            role,
            scopes,
            totp_enabled: false,
            totp_secret_ciphertext_hex: None,
            totp_secret_nonce_hex: None,
            totp_secret_salt_hex: None,
        };
        let metadata = serde_json::json!({
            "operator_id": operator.id,
            "username": operator.username,
            "role": operator.role,
            "scopes": operator.scopes,
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
                    INSERT INTO operators (id, username, password_hash, role, scopes)
                    VALUES ($1, $2, $3, $4, $5)
                    "#,
                )
                .bind(operator.id)
                .bind(&operator.username)
                .bind(&operator.password_hash)
                .bind(&operator.role)
                .bind(serde_json::json!(operator.scopes))
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
        let access_token = generate_token();
        let refresh_token = generate_token();
        let session_id = Uuid::new_v4();
        let now = unix_now();
        let expires_unix = now.saturating_add(ACCESS_TOKEN_TTL_SECS);
        let refresh_expires_unix = now.saturating_add(REFRESH_TOKEN_TTL_SECS);
        let access_hash = token_hash(&access_token);
        let refresh_hash = token_hash(&refresh_token);

        match self {
            Self::Memory(memory) => {
                memory.sessions.write().await.push(OperatorSessionRecord {
                    session_id,
                    access_token_hash: access_hash,
                    refresh_token_hash: refresh_hash,
                    operator_id: operator.id,
                    expires_unix,
                    refresh_expires_unix,
                    created_unix: now,
                    revoked: false,
                });
            }
            Self::Postgres(pool) => {
                sqlx::query(
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
                    "#,
                )
                .bind(session_id)
                .bind(operator.id)
                .bind(&access_hash)
                .bind(&refresh_hash)
                .bind(expires_unix as f64)
                .bind(refresh_expires_unix as f64)
                .execute(pool)
                .await?;
            }
        }

        Ok(AuthResponse {
            token_type: "Bearer",
            access_token,
            refresh_token,
            expires_in_secs: ACCESS_TOKEN_TTL_SECS,
            refresh_expires_in_secs: REFRESH_TOKEN_TTL_SECS,
            operator,
        })
    }
}

pub(crate) fn parse_scopes(value: serde_json::Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|scope| scope.as_str().map(ToOwned::to_owned))
        .collect()
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
