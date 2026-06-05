use std::collections::HashSet;

use anyhow::{Context, Result};
use serde_json::json;
use sqlx::Row;
use uuid::Uuid;

use crate::{
    model::{
        AuthContext, ClientKeyRevocationView, CreateClientKeyRevocationRequest,
        KeyLifecycleClientView, KeyLifecycleReportView,
    },
    repository::Repository,
    repository_enrollment::{public_key_sha256_hex, ENROLLMENT_PURPOSE_REBUILD_REENROLLMENT},
    util::unix_now,
};

#[derive(Clone, Copy, Debug)]
pub(crate) struct KeyLifecycleTrustReport {
    pub(crate) server_ed25519_public_key_configured: bool,
    pub(crate) discovery_trusted_server_key_count: usize,
    pub(crate) gateway_server_public_key_configured: bool,
}

impl Repository {
    pub(crate) async fn revoke_current_client_key(
        &self,
        client_id: &str,
        request: &CreateClientKeyRevocationRequest,
        operator: &AuthContext,
    ) -> Result<ClientKeyRevocationView> {
        let reason = normalized_reason(request.reason.as_deref());
        match self {
            Self::Memory(memory) => {
                let current_public_key = memory
                    .client_public_keys
                    .read()
                    .await
                    .get(client_id)
                    .cloned()
                    .with_context(|| format!("client public key missing for {client_id}"))?;
                let public_key_sha256_hex = public_key_sha256_hex(&current_public_key);
                if let Some(existing) = memory
                    .client_key_revocations
                    .read()
                    .await
                    .iter()
                    .find(|record| {
                        record.client_id == client_id
                            && record.public_key_sha256_hex == public_key_sha256_hex
                    })
                    .cloned()
                {
                    mark_memory_agent_revoked(memory, client_id).await;
                    return Ok(existing);
                }

                let record = ClientKeyRevocationView {
                    id: Uuid::new_v4(),
                    client_id: client_id.to_string(),
                    public_key_sha256_hex,
                    reason,
                    revoked_by: Some(operator.operator.id),
                    created_at: unix_now().to_string(),
                };
                memory
                    .client_key_revocations
                    .write()
                    .await
                    .push(record.clone());
                mark_memory_agent_revoked(memory, client_id).await;
                memory
                    .audits
                    .write()
                    .await
                    .push(crate::model::AuditLogView {
                        id: Uuid::new_v4(),
                        actor_id: Some(operator.operator.id),
                        action: "client_key.revoked".to_string(),
                        target: format!("client:{client_id}"),
                        command_hash: None,
                        metadata: json!({
                            "client_id": client_id,
                            "public_key_sha256_hex": record.public_key_sha256_hex,
                            "reason": record.reason,
                        }),
                        created_at: unix_now().to_string(),
                    });
                Ok(record)
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let row = sqlx::query(
                    r#"
                    SELECT public_key
                    FROM clients
                    WHERE id = $1
                    FOR UPDATE
                    "#,
                )
                .bind(client_id)
                .fetch_optional(&mut *tx)
                .await?;
                let Some(row) = row else {
                    anyhow::bail!("client not found: {client_id}");
                };
                let current_public_key: Vec<u8> = row.try_get("public_key")?;
                if current_public_key.is_empty() {
                    anyhow::bail!("client public key missing for {client_id}");
                }
                let public_key_sha256_hex = public_key_sha256_hex(&current_public_key);
                if let Some(existing) =
                    fetch_postgres_key_revocation(&mut tx, client_id, &public_key_sha256_hex)
                        .await?
                {
                    sqlx::query("UPDATE clients SET status = 'revoked' WHERE id = $1")
                        .bind(client_id)
                        .execute(&mut *tx)
                        .await?;
                    tx.commit().await?;
                    return Ok(existing);
                }

                let id = Uuid::new_v4();
                sqlx::query(
                    r#"
                    INSERT INTO client_key_revocations (
                        id, client_id, public_key_sha256_hex, reason, revoked_by
                    )
                    VALUES ($1, $2, $3, $4, $5)
                    "#,
                )
                .bind(id)
                .bind(client_id)
                .bind(&public_key_sha256_hex)
                .bind(&reason)
                .bind(operator.operator.id)
                .execute(&mut *tx)
                .await?;
                sqlx::query("UPDATE clients SET status = 'revoked' WHERE id = $1")
                    .bind(client_id)
                    .execute(&mut *tx)
                    .await?;
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (
                        id, actor_id, action, target, command_hash, metadata
                    )
                    VALUES ($1, $2, 'client_key.revoked', $3, NULL, $4)
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind(operator.operator.id)
                .bind(format!("client:{client_id}"))
                .bind(json!({
                    "client_id": client_id,
                    "public_key_sha256_hex": public_key_sha256_hex,
                    "reason": reason,
                }))
                .execute(&mut *tx)
                .await?;
                let record =
                    fetch_postgres_key_revocation(&mut tx, client_id, &public_key_sha256_hex)
                        .await?
                        .context("inserted client key revocation was not readable")?;
                tx.commit().await?;
                Ok(record)
            }
        }
    }

    pub(crate) async fn list_client_key_revocations(
        &self,
        limit: i64,
    ) -> Result<Vec<ClientKeyRevocationView>> {
        match self {
            Self::Memory(memory) => {
                let mut records = memory.client_key_revocations.read().await.clone();
                records.sort_by(|left, right| right.created_at.cmp(&left.created_at));
                records.truncate(limit as usize);
                Ok(records)
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        id,
                        client_id,
                        public_key_sha256_hex,
                        reason,
                        revoked_by,
                        EXTRACT(EPOCH FROM created_at)::bigint AS created_unix
                    FROM client_key_revocations
                    ORDER BY created_at DESC
                    LIMIT $1
                    "#,
                )
                .bind(limit)
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(client_key_revocation_from_row)
                    .collect()
            }
        }
    }

    pub(crate) async fn key_lifecycle_report(
        &self,
        trust: KeyLifecycleTrustReport,
    ) -> Result<KeyLifecycleReportView> {
        match self {
            Self::Memory(memory) => {
                let agents = memory.agents.read().await.clone();
                let public_keys = memory.client_public_keys.read().await.clone();
                let revocations = memory.client_key_revocations.read().await.clone();
                let revoked_current_keys = revoked_key_set(&revocations);
                let tokens = memory.enrollment_tokens.read().await.clone();
                let now = unix_now();
                let mut clients = agents
                    .into_iter()
                    .map(|agent| {
                        let current_public_key_sha256_hex =
                            public_keys.get(&agent.id).and_then(|key| {
                                (!key.is_empty()).then(|| public_key_sha256_hex(key.as_slice()))
                            });
                        let latest = latest_current_revocation(
                            &revocations,
                            &agent.id,
                            current_public_key_sha256_hex.as_deref(),
                        );
                        KeyLifecycleClientView {
                            client_id: agent.id,
                            display_name: agent.display_name,
                            status: agent.status,
                            current_key_revoked: latest.is_some(),
                            current_public_key_sha256_hex,
                            latest_revoked_at: latest.map(|record| record.created_at.clone()),
                            latest_revocation_reason: latest
                                .and_then(|record| record.reason.clone()),
                        }
                    })
                    .collect::<Vec<_>>();
                clients.sort_by(|left, right| left.client_id.cmp(&right.client_id));
                let rebuild_reenrollment_token_count = tokens
                    .iter()
                    .filter(|token| token.purpose == ENROLLMENT_PURPOSE_REBUILD_REENROLLMENT)
                    .count();
                let active_rebuild_reenrollment_token_count = tokens
                    .iter()
                    .filter(|token| {
                        token.purpose == ENROLLMENT_PURPOSE_REBUILD_REENROLLMENT
                            && token.used_at_unix.is_none()
                            && token.expires_unix > now
                    })
                    .count();
                Ok(KeyLifecycleReportView {
                    server_ed25519_public_key_configured: trust
                        .server_ed25519_public_key_configured,
                    discovery_trusted_server_key_count: trust.discovery_trusted_server_key_count,
                    gateway_server_public_key_configured: trust
                        .gateway_server_public_key_configured,
                    enrolled_client_count: clients
                        .iter()
                        .filter(|client| client.current_public_key_sha256_hex.is_some())
                        .count(),
                    current_key_revoked_count: public_keys
                        .iter()
                        .filter(|(client_id, key)| {
                            revoked_current_keys
                                .contains(&(client_id.to_string(), public_key_sha256_hex(key)))
                        })
                        .count(),
                    revocation_count: revocations.len(),
                    rebuild_reenrollment_token_count,
                    active_rebuild_reenrollment_token_count,
                    clients,
                })
            }
            Self::Postgres(pool) => {
                let client_rows = sqlx::query(
                    r#"
                    SELECT id, display_name, status, public_key
                    FROM clients
                    ORDER BY id
                    LIMIT 1000
                    "#,
                )
                .fetch_all(pool)
                .await?;
                let revocation_rows = sqlx::query(
                    r#"
                    SELECT
                        id,
                        client_id,
                        public_key_sha256_hex,
                        reason,
                        revoked_by,
                        EXTRACT(EPOCH FROM created_at)::bigint AS created_unix
                    FROM client_key_revocations
                    ORDER BY created_at DESC
                    LIMIT 5000
                    "#,
                )
                .fetch_all(pool)
                .await?;
                let revocations = revocation_rows
                    .into_iter()
                    .map(client_key_revocation_from_row)
                    .collect::<Result<Vec<_>>>()?;
                let token_row = sqlx::query(
                    r#"
                    SELECT
                        COUNT(*) FILTER (WHERE purpose = 'rebuild_reenrollment') AS rebuild_count,
                        COUNT(*) FILTER (
                            WHERE purpose = 'rebuild_reenrollment'
                              AND used_at IS NULL
                              AND expires_at > now()
                        ) AS active_rebuild_count
                    FROM enrollment_tokens
                    "#,
                )
                .fetch_one(pool)
                .await?;
                let mut current_key_revoked_count = 0usize;
                let clients = client_rows
                    .into_iter()
                    .map(|row| {
                        let client_id: String = row.try_get("id")?;
                        let display_name: String = row.try_get("display_name")?;
                        let status: String = row.try_get("status")?;
                        let public_key: Vec<u8> = row.try_get("public_key")?;
                        let fingerprint =
                            (!public_key.is_empty()).then(|| public_key_sha256_hex(&public_key));
                        let latest = latest_current_revocation(
                            &revocations,
                            &client_id,
                            fingerprint.as_deref(),
                        );
                        if latest.is_some() {
                            current_key_revoked_count += 1;
                        }
                        Ok(KeyLifecycleClientView {
                            client_id,
                            display_name,
                            status,
                            current_public_key_sha256_hex: fingerprint,
                            current_key_revoked: latest.is_some(),
                            latest_revoked_at: latest.map(|record| record.created_at.clone()),
                            latest_revocation_reason: latest
                                .and_then(|record| record.reason.clone()),
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;
                let enrolled_client_count = clients
                    .iter()
                    .filter(|client| client.current_public_key_sha256_hex.is_some())
                    .count();
                Ok(KeyLifecycleReportView {
                    server_ed25519_public_key_configured: trust
                        .server_ed25519_public_key_configured,
                    discovery_trusted_server_key_count: trust.discovery_trusted_server_key_count,
                    gateway_server_public_key_configured: trust
                        .gateway_server_public_key_configured,
                    enrolled_client_count,
                    current_key_revoked_count,
                    revocation_count: revocations.len(),
                    rebuild_reenrollment_token_count: token_row
                        .try_get::<i64, _>("rebuild_count")?
                        .max(0) as usize,
                    active_rebuild_reenrollment_token_count: token_row
                        .try_get::<i64, _>("active_rebuild_count")?
                        .max(0)
                        as usize,
                    clients,
                })
            }
        }
    }

    pub(crate) async fn is_client_key_revoked(
        &self,
        client_id: &str,
        public_key: &[u8],
    ) -> Result<bool> {
        let public_key_sha256_hex = public_key_sha256_hex(public_key);
        match self {
            Self::Memory(memory) => {
                Ok(memory
                    .client_key_revocations
                    .read()
                    .await
                    .iter()
                    .any(|record| {
                        record.client_id == client_id
                            && record.public_key_sha256_hex == public_key_sha256_hex
                    }))
            }
            Self::Postgres(pool) => {
                let row = sqlx::query(
                    r#"
                    SELECT 1
                    FROM client_key_revocations
                    WHERE client_id = $1 AND public_key_sha256_hex = $2
                    LIMIT 1
                    "#,
                )
                .bind(client_id)
                .bind(public_key_sha256_hex)
                .fetch_optional(pool)
                .await?;
                Ok(row.is_some())
            }
        }
    }
}

async fn mark_memory_agent_revoked(memory: &crate::repository::MemoryState, client_id: &str) {
    if let Some(agent) = memory
        .agents
        .write()
        .await
        .iter_mut()
        .find(|agent| agent.id == client_id)
    {
        agent.status = "revoked".to_string();
    }
}

async fn fetch_postgres_key_revocation(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    client_id: &str,
    public_key_sha256_hex: &str,
) -> Result<Option<ClientKeyRevocationView>> {
    let row = sqlx::query(
        r#"
        SELECT
            id,
            client_id,
            public_key_sha256_hex,
            reason,
            revoked_by,
            EXTRACT(EPOCH FROM created_at)::bigint AS created_unix
        FROM client_key_revocations
        WHERE client_id = $1 AND public_key_sha256_hex = $2
        "#,
    )
    .bind(client_id)
    .bind(public_key_sha256_hex)
    .fetch_optional(&mut **tx)
    .await?;
    row.map(client_key_revocation_from_row).transpose()
}

fn client_key_revocation_from_row(row: sqlx::postgres::PgRow) -> Result<ClientKeyRevocationView> {
    Ok(ClientKeyRevocationView {
        id: row.try_get("id")?,
        client_id: row.try_get("client_id")?,
        public_key_sha256_hex: row.try_get("public_key_sha256_hex")?,
        reason: row.try_get("reason")?,
        revoked_by: row.try_get("revoked_by")?,
        created_at: row.try_get::<i64, _>("created_unix")?.to_string(),
    })
}

fn latest_current_revocation<'a>(
    revocations: &'a [ClientKeyRevocationView],
    client_id: &str,
    public_key_sha256_hex: Option<&str>,
) -> Option<&'a ClientKeyRevocationView> {
    let public_key_sha256_hex = public_key_sha256_hex?;
    revocations.iter().find(|record| {
        record.client_id == client_id && record.public_key_sha256_hex == public_key_sha256_hex
    })
}

fn revoked_key_set(revocations: &[ClientKeyRevocationView]) -> HashSet<(String, String)> {
    revocations
        .iter()
        .map(|record| {
            (
                record.client_id.clone(),
                record.public_key_sha256_hex.clone(),
            )
        })
        .collect::<HashSet<_>>()
}

fn normalized_reason(reason: Option<&str>) -> Option<String> {
    reason
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.chars().take(1024).collect())
}
