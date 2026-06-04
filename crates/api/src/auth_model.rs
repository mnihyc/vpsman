use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Debug)]
pub(crate) struct OperatorRecord {
    pub(crate) id: Uuid,
    pub(crate) username: String,
    pub(crate) password_hash: String,
    pub(crate) role: String,
    pub(crate) scopes: Vec<String>,
    pub(crate) totp_enabled: bool,
    pub(crate) totp_secret_ciphertext_hex: Option<String>,
    pub(crate) totp_secret_nonce_hex: Option<String>,
    pub(crate) totp_secret_salt_hex: Option<String>,
}

impl OperatorRecord {
    pub(crate) fn view(&self) -> OperatorView {
        OperatorView {
            id: self.id,
            username: self.username.clone(),
            role: self.role.clone(),
            scopes: if self.scopes.is_empty() {
                crate::security::default_operator_scopes(&self.role)
            } else {
                self.scopes.clone()
            },
            totp_enabled: self.totp_enabled,
        }
    }

    pub(crate) fn encrypted_totp_secret(&self) -> Option<crate::auth_totp::EncryptedTotpSecret> {
        Some(crate::auth_totp::EncryptedTotpSecret {
            ciphertext_hex: self.totp_secret_ciphertext_hex.clone()?,
            nonce_hex: self.totp_secret_nonce_hex.clone()?,
            salt_hex: self.totp_secret_salt_hex.clone()?,
        })
    }
}

#[derive(Clone, Debug)]
pub(crate) struct OperatorSessionRecord {
    pub(crate) session_id: Uuid,
    pub(crate) access_token_hash: String,
    pub(crate) refresh_token_hash: String,
    pub(crate) operator_id: Uuid,
    pub(crate) expires_unix: u64,
    pub(crate) refresh_expires_unix: u64,
    pub(crate) created_unix: u64,
    pub(crate) revoked: bool,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct OperatorView {
    pub(crate) id: Uuid,
    pub(crate) username: String,
    pub(crate) role: String,
    pub(crate) scopes: Vec<String>,
    pub(crate) totp_enabled: bool,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct OperatorSessionView {
    pub(crate) id: Uuid,
    pub(crate) operator_id: Uuid,
    pub(crate) operator_username: String,
    pub(crate) operator_role: String,
    pub(crate) current: bool,
    pub(crate) created_at: String,
    pub(crate) expires_at: String,
    pub(crate) refresh_expires_at: String,
    pub(crate) revoked: bool,
    pub(crate) revoked_at: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct AuthContext {
    pub(crate) operator: OperatorView,
    pub(crate) session_id: Uuid,
}

#[derive(Debug, Deserialize)]
pub(crate) struct BootstrapOperatorRequest {
    pub(crate) username: String,
    pub(crate) password: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct LoginRequest {
    pub(crate) username: String,
    pub(crate) password: String,
    pub(crate) totp_code: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CreateOperatorRequest {
    pub(crate) username: String,
    pub(crate) password: String,
    pub(crate) role: String,
    #[serde(default)]
    pub(crate) scopes: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct RefreshRequest {
    pub(crate) refresh_token: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct AuthResponse {
    pub(crate) token_type: &'static str,
    pub(crate) access_token: String,
    pub(crate) refresh_token: String,
    pub(crate) expires_in_secs: u64,
    pub(crate) refresh_expires_in_secs: u64,
    pub(crate) operator: OperatorView,
}

#[derive(Debug, Deserialize)]
pub(crate) struct TotpSetupRequest {
    pub(crate) password: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct TotpSetupResponse {
    pub(crate) operator_id: Uuid,
    pub(crate) secret_base32: String,
    pub(crate) otpauth_uri: String,
    pub(crate) algorithm: &'static str,
    pub(crate) digits: u8,
    pub(crate) period_secs: u64,
}

#[derive(Debug, Deserialize)]
pub(crate) struct TotpConfirmRequest {
    pub(crate) password: String,
    pub(crate) code: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct TotpDisableRequest {
    pub(crate) password: String,
    pub(crate) code: String,
}

#[derive(Debug)]
pub(crate) enum TotpSetupOutcome {
    Created(TotpSetupResponse),
    AlreadyEnabled,
    InvalidPassword,
    OperatorMissing,
}

#[derive(Debug)]
pub(crate) enum TotpUpdateOutcome {
    Updated(OperatorView),
    InvalidCredentials,
    NotConfigured,
    OperatorMissing,
}
