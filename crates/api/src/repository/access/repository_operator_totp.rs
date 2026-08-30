use anyhow::{Context, Result};
use sqlx::Row;
use uuid::Uuid;

use crate::{
    auth_totp::{
        base32_no_padding, decrypt_totp_secret, encrypt_new_totp_secret, matching_totp_step,
        otpauth_uri, TOTP_DIGITS, TOTP_PERIOD_SECS,
    },
    model::{
        AuthContext, OperatorRecord, OperatorView, TotpSetupOutcome, TotpSetupResponse,
        TotpUpdateOutcome,
    },
    repository::Repository,
    repository_auth::{parse_operator_preferences, parse_scopes, postgres_totp_step},
    unix_now, verify_operator_password, DEFAULT_REFRESH_TOKEN_TTL_SECS,
};

impl Repository {
    pub(crate) async fn setup_operator_totp(
        &self,
        actor: &AuthContext,
        password: &str,
    ) -> Result<TotpSetupOutcome> {
        match self {
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
                if let Some(secret) = existing_pending_totp_secret(&operator, password)? {
                    return Ok(TotpSetupOutcome::Created(setup_response(
                        &operator.view(),
                        &secret,
                    )));
                }
                let (secret, encrypted) = encrypt_new_totp_secret(password)?;
                sqlx::query(
                    r#"
                    UPDATE operators
                    SET
                        totp_enabled = false,
                        totp_secret_ciphertext_hex = $2,
                        totp_secret_nonce_hex = $3,
                        totp_secret_salt_hex = $4,
                        totp_last_accepted_step = NULL
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
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let Some(operator) = select_operator_for_update(&mut tx, actor.operator.id).await?
                else {
                    return Ok(TotpUpdateOutcome::OperatorMissing);
                };
                if operator.encrypted_totp_secret().is_none() {
                    return Ok(TotpUpdateOutcome::NotConfigured);
                }
                if enable && operator.totp_enabled {
                    return Ok(TotpUpdateOutcome::AlreadyEnabled);
                }
                let Some(matched_step) = matching_operator_totp_step(&operator, password, code)?
                else {
                    return Ok(TotpUpdateOutcome::InvalidCredentials);
                };
                if operator
                    .totp_last_accepted_step
                    .is_some_and(|last_step| matched_step <= last_step)
                {
                    return Ok(TotpUpdateOutcome::InvalidCredentials);
                }
                let view = if enable {
                    sqlx::query(
                        r#"
                        UPDATE operators
                        SET
                            totp_enabled = true,
                            totp_last_accepted_step = $2
                        WHERE id = $1
                        "#,
                    )
                    .bind(operator.id)
                    .bind(matched_step as i64)
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
                            totp_secret_salt_hex = NULL,
                            totp_last_accepted_step = NULL
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

fn existing_pending_totp_secret(
    operator: &OperatorRecord,
    password: &str,
) -> Result<Option<Vec<u8>>> {
    anyhow::ensure!(
        operator.totp_last_accepted_step.is_none(),
        "disabled operator has inconsistent TOTP replay state"
    );
    match [
        operator.totp_secret_ciphertext_hex.is_some(),
        operator.totp_secret_nonce_hex.is_some(),
        operator.totp_secret_salt_hex.is_some(),
    ] {
        [false, false, false] => Ok(None),
        [true, true, true] => {
            let encrypted = operator
                .encrypted_totp_secret()
                .context("complete pending TOTP material could not be read")?;
            let secret = decrypt_totp_secret(password, &encrypted)
                .context("stored pending TOTP secret is corrupt")?;
            anyhow::ensure!(
                (16..=64).contains(&secret.len()),
                "stored pending TOTP secret length is invalid"
            );
            Ok(Some(secret))
        }
        _ => anyhow::bail!("stored pending TOTP secret material is incomplete"),
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

fn matching_operator_totp_step(
    operator: &OperatorRecord,
    password: &str,
    code: &str,
) -> Result<Option<u64>> {
    if !verify_operator_password(password, &operator.password_hash)? {
        return Ok(None);
    }
    let Some(encrypted) = operator.encrypted_totp_secret() else {
        return Ok(None);
    };
    let secret = match decrypt_totp_secret(password, &encrypted) {
        Ok(secret) => secret,
        Err(_) => return Ok(None),
    };
    Ok(matching_totp_step(&secret, code, unix_now()))
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
        "operator_role": actor.operator.role,
        "operator_session_id": actor.audit_session_id(),
        "totp_status": status,
        "result": "succeeded",
        "origin_kind": "operator_request",
        "component": "operator-totp",
    }))
    .execute(&mut **tx)
    .await?;
    Ok(())
}
