use anyhow::Result;
use sqlx::Row;
use uuid::Uuid;

use crate::{
    auth_totp::{
        base32_no_padding, decrypt_totp_secret, encrypt_new_totp_secret, otpauth_uri,
        verify_totp_code, TOTP_DIGITS, TOTP_PERIOD_SECS,
    },
    model::{
        AuditLogView, AuthContext, OperatorRecord, OperatorView, TotpSetupOutcome,
        TotpSetupResponse, TotpUpdateOutcome,
    },
    repository::Repository,
    repository_auth::{parse_operator_preferences, parse_scopes},
    unix_now, verify_operator_password,
};

impl Repository {
    pub(crate) async fn setup_operator_totp(
        &self,
        actor: &AuthContext,
        password: &str,
    ) -> Result<TotpSetupOutcome> {
        match self {
            Self::Memory(memory) => {
                let mut operators = memory.operators.write().await;
                let Some(operator) = operators
                    .iter_mut()
                    .find(|operator| operator.id == actor.operator.id)
                else {
                    return Ok(TotpSetupOutcome::OperatorMissing);
                };
                if operator.totp_enabled {
                    return Ok(TotpSetupOutcome::AlreadyEnabled);
                }
                if !verify_operator_password(password, &operator.password_hash)? {
                    return Ok(TotpSetupOutcome::InvalidPassword);
                }
                let (secret, encrypted) = encrypt_new_totp_secret(password)?;
                operator.totp_secret_ciphertext_hex = Some(encrypted.ciphertext_hex);
                operator.totp_secret_nonce_hex = Some(encrypted.nonce_hex);
                operator.totp_secret_salt_hex = Some(encrypted.salt_hex);
                let response = setup_response(&operator.view(), &secret);
                drop(operators);
                record_totp_audit(memory, actor, "operator_totp.setup", "pending").await;
                Ok(TotpSetupOutcome::Created(response))
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let Some(operator) = select_operator_for_update(&mut tx, actor.operator.id).await?
                else {
                    return Ok(TotpSetupOutcome::OperatorMissing);
                };
                if operator.totp_enabled {
                    return Ok(TotpSetupOutcome::AlreadyEnabled);
                }
                if !verify_operator_password(password, &operator.password_hash)? {
                    return Ok(TotpSetupOutcome::InvalidPassword);
                }
                let (secret, encrypted) = encrypt_new_totp_secret(password)?;
                sqlx::query(
                    r#"
                    UPDATE operators
                    SET
                        totp_enabled = false,
                        totp_secret_ciphertext_hex = $2,
                        totp_secret_nonce_hex = $3,
                        totp_secret_salt_hex = $4
                    WHERE id = $1
                    "#,
                )
                .bind(operator.id)
                .bind(&encrypted.ciphertext_hex)
                .bind(&encrypted.nonce_hex)
                .bind(&encrypted.salt_hex)
                .execute(&mut *tx)
                .await?;
                insert_totp_audit(&mut tx, actor, "operator_totp.setup", "pending").await?;
                tx.commit().await?;
                Ok(TotpSetupOutcome::Created(setup_response(
                    &operator.view(),
                    &secret,
                )))
            }
        }
    }

    pub(crate) async fn confirm_operator_totp(
        &self,
        actor: &AuthContext,
        password: &str,
        code: &str,
    ) -> Result<TotpUpdateOutcome> {
        self.update_operator_totp(actor, password, code, true).await
    }

    pub(crate) async fn disable_operator_totp(
        &self,
        actor: &AuthContext,
        password: &str,
        code: &str,
    ) -> Result<TotpUpdateOutcome> {
        self.update_operator_totp(actor, password, code, false)
            .await
    }

    async fn update_operator_totp(
        &self,
        actor: &AuthContext,
        password: &str,
        code: &str,
        enable: bool,
    ) -> Result<TotpUpdateOutcome> {
        match self {
            Self::Memory(memory) => {
                let mut operators = memory.operators.write().await;
                let Some(operator) = operators
                    .iter_mut()
                    .find(|operator| operator.id == actor.operator.id)
                else {
                    return Ok(TotpUpdateOutcome::OperatorMissing);
                };
                if operator.encrypted_totp_secret().is_none() {
                    return Ok(TotpUpdateOutcome::NotConfigured);
                }
                if !verify_totp_operator_code(operator, password, code)? {
                    return Ok(TotpUpdateOutcome::InvalidCredentials);
                }
                if enable {
                    operator.totp_enabled = true;
                } else {
                    operator.totp_enabled = false;
                    operator.totp_secret_ciphertext_hex = None;
                    operator.totp_secret_nonce_hex = None;
                    operator.totp_secret_salt_hex = None;
                }
                let view = operator.view();
                drop(operators);
                record_totp_audit(
                    memory,
                    actor,
                    if enable {
                        "operator_totp.enabled"
                    } else {
                        "operator_totp.disabled"
                    },
                    if enable { "enabled" } else { "disabled" },
                )
                .await;
                Ok(TotpUpdateOutcome::Updated(Box::new(view)))
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let Some(operator) = select_operator_for_update(&mut tx, actor.operator.id).await?
                else {
                    return Ok(TotpUpdateOutcome::OperatorMissing);
                };
                if operator.encrypted_totp_secret().is_none() {
                    return Ok(TotpUpdateOutcome::NotConfigured);
                }
                if !verify_totp_operator_code(&operator, password, code)? {
                    return Ok(TotpUpdateOutcome::InvalidCredentials);
                }
                let view = if enable {
                    sqlx::query(
                        r#"
                        UPDATE operators
                        SET totp_enabled = true
                        WHERE id = $1
                        "#,
                    )
                    .bind(operator.id)
                    .execute(&mut *tx)
                    .await?;
                    OperatorView {
                        totp_enabled: true,
                        ..operator.view()
                    }
                } else {
                    sqlx::query(
                        r#"
                        UPDATE operators
                        SET
                            totp_enabled = false,
                            totp_secret_ciphertext_hex = NULL,
                            totp_secret_nonce_hex = NULL,
                            totp_secret_salt_hex = NULL
                        WHERE id = $1
                        "#,
                    )
                    .bind(operator.id)
                    .execute(&mut *tx)
                    .await?;
                    OperatorView {
                        totp_enabled: false,
                        ..operator.view()
                    }
                };
                insert_totp_audit(
                    &mut tx,
                    actor,
                    if enable {
                        "operator_totp.enabled"
                    } else {
                        "operator_totp.disabled"
                    },
                    if enable { "enabled" } else { "disabled" },
                )
                .await?;
                tx.commit().await?;
                Ok(TotpUpdateOutcome::Updated(Box::new(view)))
            }
        }
    }
}

fn setup_response(operator: &OperatorView, secret: &[u8]) -> TotpSetupResponse {
    let secret_base32 = base32_no_padding(secret);
    TotpSetupResponse {
        operator_id: operator.id,
        otpauth_uri: otpauth_uri(&operator.username, &secret_base32),
        secret_base32,
        algorithm: "SHA1",
        digits: TOTP_DIGITS,
        period_secs: TOTP_PERIOD_SECS,
    }
}

fn verify_totp_operator_code(
    operator: &OperatorRecord,
    password: &str,
    code: &str,
) -> Result<bool> {
    if !verify_operator_password(password, &operator.password_hash)? {
        return Ok(false);
    }
    let Some(encrypted) = operator.encrypted_totp_secret() else {
        return Ok(false);
    };
    let secret = match decrypt_totp_secret(password, &encrypted) {
        Ok(secret) => secret,
        Err(_) => return Ok(false),
    };
    Ok(verify_totp_code(&secret, code, unix_now()))
}

async fn select_operator_for_update(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    operator_id: Uuid,
) -> Result<Option<OperatorRecord>> {
    let row = sqlx::query(
        r#"
        SELECT
            id,
            username,
            password_hash,
            role,
            scopes,
            preferences,
            totp_enabled,
            totp_secret_ciphertext_hex,
            totp_secret_nonce_hex,
            totp_secret_salt_hex
        FROM operators
        WHERE id = $1
        FOR UPDATE
        "#,
    )
    .bind(operator_id)
    .fetch_optional(&mut **tx)
    .await?;
    row.map(|row| {
        Ok(OperatorRecord {
            id: row.try_get("id")?,
            username: row.try_get("username")?,
            password_hash: row.try_get("password_hash")?,
            role: row.try_get("role")?,
            scopes: parse_scopes(row.try_get("scopes")?),
            preferences: parse_operator_preferences(row.try_get("preferences")?),
            totp_enabled: row.try_get("totp_enabled")?,
            totp_secret_ciphertext_hex: row.try_get("totp_secret_ciphertext_hex")?,
            totp_secret_nonce_hex: row.try_get("totp_secret_nonce_hex")?,
            totp_secret_salt_hex: row.try_get("totp_secret_salt_hex")?,
        })
    })
    .transpose()
}

async fn record_totp_audit(
    memory: &crate::repository::MemoryState,
    actor: &AuthContext,
    action: &str,
    status: &str,
) {
    memory.audits.write().await.push(AuditLogView {
        id: Uuid::new_v4(),
        actor_id: Some(actor.operator.id),
        action: action.to_string(),
        target: format!("operator:{}", actor.operator.id),
        command_hash: None,
        metadata: serde_json::json!({
            "operator_id": actor.operator.id,
            "operator_username": actor.operator.username,
            "session_id": actor.session_id,
            "status": status,
        }),
        created_at: unix_now().to_string(),
    });
}

async fn insert_totp_audit(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    actor: &AuthContext,
    action: &str,
    status: &str,
) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO audit_logs (id, actor_id, action, target, command_hash, metadata)
        VALUES ($1, $2, $3, $4, NULL, $5)
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(actor.operator.id)
    .bind(action)
    .bind(format!("operator:{}", actor.operator.id))
    .bind(serde_json::json!({
        "operator_id": actor.operator.id,
        "operator_username": actor.operator.username,
        "session_id": actor.session_id,
        "status": status,
    }))
    .execute(&mut **tx)
    .await?;
    Ok(())
}
