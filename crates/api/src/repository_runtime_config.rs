use anyhow::Result;
use sqlx::Row;
use uuid::Uuid;

use crate::{
    model::{AuthContext, RuntimeConfigOverrideView},
    repository::Repository,
    unix_now,
};

impl Repository {
    pub(crate) async fn list_runtime_config_overrides(
        &self,
        client_id: Option<&str>,
    ) -> Result<Vec<RuntimeConfigOverrideView>> {
        match self {
            Self::Memory(memory) => {
                let mut overrides = memory.runtime_config_overrides.read().await.clone();
                if let Some(client_id) = client_id {
                    overrides.retain(|override_record| override_record.client_id == client_id);
                }
                overrides.sort_by(|left, right| left.client_id.cmp(&right.client_id));
                Ok(overrides)
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT client_id, toml, reason, updated_at::text AS updated_at, updated_by
                    FROM client_runtime_config_overrides
                    WHERE ($1::text IS NULL OR client_id = $1)
                    ORDER BY client_id
                    "#,
                )
                .bind(client_id)
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(|row| {
                        Ok(RuntimeConfigOverrideView {
                            client_id: row.try_get("client_id")?,
                            toml: row.try_get("toml")?,
                            reason: row.try_get("reason")?,
                            updated_at: row.try_get("updated_at")?,
                            updated_by: row.try_get("updated_by")?,
                        })
                    })
                    .collect()
            }
        }
    }

    pub(crate) async fn upsert_runtime_config_overrides(
        &self,
        client_ids: &[String],
        toml: &str,
        reason: &str,
        operator: &AuthContext,
    ) -> Result<Vec<RuntimeConfigOverrideView>> {
        match self {
            Self::Memory(memory) => {
                let now = unix_now().to_string();
                let mut overrides = memory.runtime_config_overrides.write().await;
                for client_id in client_ids {
                    if let Some(existing) = overrides
                        .iter_mut()
                        .find(|override_record| override_record.client_id == *client_id)
                    {
                        existing.toml = toml.to_string();
                        existing.reason = reason.to_string();
                        existing.updated_at = now.clone();
                        existing.updated_by = Some(operator.operator.id);
                    } else {
                        overrides.push(RuntimeConfigOverrideView {
                            client_id: client_id.clone(),
                            toml: toml.to_string(),
                            reason: reason.to_string(),
                            updated_at: now.clone(),
                            updated_by: Some(operator.operator.id),
                        });
                    }
                }
                Ok(overrides
                    .iter()
                    .filter(|override_record| client_ids.contains(&override_record.client_id))
                    .cloned()
                    .collect())
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                for client_id in client_ids {
                    sqlx::query(
                        r#"
                        INSERT INTO client_runtime_config_overrides (
                            client_id, toml, reason, updated_by, updated_at
                        )
                        VALUES ($1, $2, $3, $4, now())
                        ON CONFLICT (client_id)
                        DO UPDATE SET
                            toml = EXCLUDED.toml,
                            reason = EXCLUDED.reason,
                            updated_by = EXCLUDED.updated_by,
                            updated_at = now()
                        "#,
                    )
                    .bind(client_id)
                    .bind(toml)
                    .bind(reason)
                    .bind(operator.operator.id)
                    .execute(&mut *tx)
                    .await?;
                    sqlx::query(
                        r#"
                        INSERT INTO audit_logs (id, actor_id, action, target, metadata)
                        VALUES ($1, $2, 'runtime_config.client_patch_upserted', $3, $4)
                        "#,
                    )
                    .bind(Uuid::new_v4())
                    .bind(operator.operator.id)
                    .bind(format!("client:{client_id}"))
                    .bind(serde_json::json!({
                        "client_id": client_id,
                        "reason": reason,
                    }))
                    .execute(&mut *tx)
                    .await?;
                }
                tx.commit().await?;
                self.list_runtime_config_overrides(None)
                    .await
                    .map(|records| {
                        records
                            .into_iter()
                            .filter(|record| client_ids.contains(&record.client_id))
                            .collect()
                    })
            }
        }
    }
}
