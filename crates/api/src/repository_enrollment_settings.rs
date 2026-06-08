use anyhow::Result;
use uuid::Uuid;

use crate::{
    model::{AuditLogView, AuthContext},
    repository::Repository,
    state::EnrollmentSettings,
    unix_now,
};

impl Repository {
    pub(crate) async fn load_enrollment_settings(
        &self,
        defaults: &EnrollmentSettings,
    ) -> Result<EnrollmentSettings> {
        match self {
            Self::Memory(memory) => Ok(memory
                .enrollment_settings
                .read()
                .await
                .clone()
                .unwrap_or_else(|| defaults.clone())),
            Self::Postgres(pool) => {
                let row: Option<(serde_json::Value,)> = sqlx::query_as(
                    r#"
                    SELECT settings
                    FROM enrollment_runtime_settings
                    WHERE id = TRUE
                    "#,
                )
                .fetch_optional(pool)
                .await?;
                let Some((settings,)) = row else {
                    return Ok(defaults.clone());
                };
                Ok(serde_json::from_value(settings)?)
            }
        }
    }

    pub(crate) async fn upsert_enrollment_settings(
        &self,
        settings: &EnrollmentSettings,
        operator: &AuthContext,
    ) -> Result<EnrollmentSettings> {
        match self {
            Self::Memory(memory) => {
                *memory.enrollment_settings.write().await = Some(settings.clone());
                memory.audits.write().await.push(enrollment_settings_audit(
                    settings,
                    Some(operator.operator.id),
                ));
                Ok(settings.clone())
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                sqlx::query(
                    r#"
                    INSERT INTO enrollment_runtime_settings (
                        id, settings, updated_by, updated_at
                    )
                    VALUES (TRUE, $1, $2, now())
                    ON CONFLICT (id) DO UPDATE
                    SET settings = EXCLUDED.settings,
                        updated_by = EXCLUDED.updated_by,
                        updated_at = now()
                    "#,
                )
                .bind(serde_json::to_value(settings)?)
                .bind(operator.operator.id)
                .execute(&mut *tx)
                .await?;
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (
                        id, actor_id, action, target, command_hash, metadata
                    )
                    VALUES ($1, $2, 'enrollment.runtime_settings_updated', 'enrollment:runtime', NULL, $3)
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind(operator.operator.id)
                .bind(enrollment_settings_metadata(settings))
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                Ok(settings.clone())
            }
        }
    }
}

fn enrollment_settings_audit(
    settings: &EnrollmentSettings,
    actor_id: Option<Uuid>,
) -> AuditLogView {
    AuditLogView {
        id: Uuid::new_v4(),
        actor_id,
        action: "enrollment.runtime_settings_updated".to_string(),
        target: "enrollment:runtime".to_string(),
        command_hash: None,
        metadata: enrollment_settings_metadata(settings),
        created_at: unix_now().to_string(),
    }
}

fn enrollment_settings_metadata(settings: &EnrollmentSettings) -> serde_json::Value {
    serde_json::json!({
        "tcp_endpoints": settings.tcp_endpoints,
        "discovery_url": settings.discovery_url,
        "gateway_server_public_key_configured": settings.gateway_server_public_key_hex.is_some(),
        "discovery_trusted_server_key_count": settings.discovery_trusted_server_ed25519_public_keys_hex.len(),
        "gateway_retry_secs": settings.gateway_retry_secs,
        "gateway_connect_timeout_secs": settings.gateway_connect_timeout_secs,
        "telemetry_light_secs": settings.telemetry_light_secs,
        "telemetry_full_secs": settings.telemetry_full_secs,
        "unmanaged_update_enabled": settings.update.unmanaged_enabled,
        "unmanaged_update_version_url": settings.update.unmanaged_version_url,
        "unmanaged_update_interval_secs": settings.update.unmanaged_interval_secs,
        "unmanaged_update_jitter_secs": settings.update.unmanaged_jitter_secs,
    })
}
