use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Debug)]
pub(crate) struct OperatorRecord {
    pub(crate) id: Uuid,
    pub(crate) username: String,
    pub(crate) password_hash: String,
    pub(crate) role: String,
    pub(crate) scopes: Vec<String>,
    pub(crate) preferences: OperatorPreferences,
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
            preferences: self.preferences.clone().normalized(),
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

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct OperatorPreferences {
    #[serde(default = "default_vps_name_display_mode")]
    pub(crate) vps_name_display_mode: String,
    #[serde(default)]
    pub(crate) timezone: Option<String>,
    #[serde(default = "default_operator_language")]
    pub(crate) language: String,
    #[serde(default = "default_show_country_flags")]
    pub(crate) show_country_flags: bool,
    #[serde(default = "default_sidebar_subpanel_default")]
    pub(crate) sidebar_subpanel_default: String,
    #[serde(default)]
    pub(crate) dashboard_curve_exclusions: Vec<String>,
    #[serde(default = "default_dashboard_top_limit")]
    pub(crate) dashboard_resource_top_limit: u8,
    #[serde(default = "default_dashboard_top_limit")]
    pub(crate) dashboard_network_top_limit: u8,
    #[serde(default = "default_bulk_output_compare_mode")]
    pub(crate) bulk_output_compare_mode: String,
    #[serde(default = "default_enrollment_install_command_template")]
    pub(crate) enrollment_install_command_template: String,
}

impl Default for OperatorPreferences {
    fn default() -> Self {
        Self {
            vps_name_display_mode: default_vps_name_display_mode(),
            timezone: None,
            language: default_operator_language(),
            show_country_flags: default_show_country_flags(),
            sidebar_subpanel_default: default_sidebar_subpanel_default(),
            dashboard_curve_exclusions: Vec::new(),
            dashboard_resource_top_limit: default_dashboard_top_limit(),
            dashboard_network_top_limit: default_dashboard_top_limit(),
            bulk_output_compare_mode: default_bulk_output_compare_mode(),
            enrollment_install_command_template: default_enrollment_install_command_template(),
        }
    }
}

impl OperatorPreferences {
    pub(crate) fn normalized(self) -> Self {
        Self {
            vps_name_display_mode: normalize_choice(
                self.vps_name_display_mode,
                "name_id_suffix",
                &["name", "name_id_suffix"],
            ),
            timezone: self.timezone.and_then(normalize_operator_timezone),
            language: normalize_choice(self.language, "en", &["en"]),
            show_country_flags: self.show_country_flags,
            sidebar_subpanel_default: normalize_choice(
                self.sidebar_subpanel_default,
                "active",
                &["active", "all"],
            ),
            dashboard_curve_exclusions: normalize_dashboard_curve_exclusions(
                self.dashboard_curve_exclusions,
            ),
            dashboard_resource_top_limit: normalize_dashboard_top_limit(
                self.dashboard_resource_top_limit,
            ),
            dashboard_network_top_limit: normalize_dashboard_top_limit(
                self.dashboard_network_top_limit,
            ),
            bulk_output_compare_mode: normalize_choice(
                self.bulk_output_compare_mode,
                "binary",
                &["binary", "text"],
            ),
            enrollment_install_command_template: normalize_enrollment_install_command_template(
                self.enrollment_install_command_template,
            ),
        }
    }
}

pub(crate) fn normalize_operator_timezone(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() || !is_valid_operator_timezone(trimmed) {
        None
    } else {
        Some(trimmed.to_string())
    }
}

pub(crate) fn is_valid_operator_timezone(timezone: &str) -> bool {
    let timezone = timezone.trim();
    !timezone.is_empty() && timezone.len() <= 64 && timezone.parse::<chrono_tz::Tz>().is_ok()
}

fn normalize_choice(value: String, fallback: &str, allowed: &[&str]) -> String {
    let trimmed = value.trim();
    if allowed.contains(&trimmed) {
        trimmed.to_string()
    } else {
        fallback.to_string()
    }
}

fn default_vps_name_display_mode() -> String {
    "name_id_suffix".to_string()
}

fn default_operator_language() -> String {
    "en".to_string()
}

fn default_show_country_flags() -> bool {
    true
}

fn default_sidebar_subpanel_default() -> String {
    "active".to_string()
}

fn default_dashboard_top_limit() -> u8 {
    8
}

fn default_bulk_output_compare_mode() -> String {
    "binary".to_string()
}

pub(crate) fn default_enrollment_install_command_template() -> String {
    "curl -fsSL https://raw.githubusercontent.com/mnihyc/vpsman/main/deploy/enroll-agent.sh | env VPSMAN_INSTALL_MODE={INSTALL_MODE} VPSMAN_ENROLLMENT_API_URL={API_URL} VPSMAN_ENROLLMENT_TOKEN={TOKEN} bash".to_string()
}

fn normalize_dashboard_top_limit(value: u8) -> u8 {
    value.clamp(3, 16)
}

fn normalize_dashboard_curve_exclusions(values: Vec<String>) -> Vec<String> {
    let mut normalized = Vec::new();
    for value in values {
        let trimmed = value.trim();
        if trimmed.is_empty()
            || trimmed.len() > 128
            || normalized.iter().any(|stored| stored == trimmed)
            || normalized.len() >= 50
        {
            continue;
        }
        normalized.push(trimmed.to_string());
    }
    normalized
}

fn normalize_enrollment_install_command_template(value: String) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        default_enrollment_install_command_template()
    } else {
        trimmed.to_string()
    }
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct OperatorView {
    pub(crate) id: Uuid,
    pub(crate) username: String,
    pub(crate) role: String,
    pub(crate) scopes: Vec<String>,
    pub(crate) preferences: OperatorPreferences,
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
    Updated(Box<OperatorView>),
    InvalidCredentials,
    NotConfigured,
    OperatorMissing,
}
