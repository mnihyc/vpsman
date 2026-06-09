use anyhow::{Context, Result};
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::Row;
use uuid::Uuid;

use crate::{
    model::{
        AgentIdentityView, AgentView, AuthContext, AuditLogView, ClientKeyRevocationView,
        CreateClientKeyRevocationRequest, KeyLifecycleClientView, KeyLifecycleReportView,
        UpsertAgentIdentityRequest,
    },
    repository::Repository,
    util::unix_now,
};

#[derive(Clone, Copy, Debug)]
pub(crate) struct KeyLifecycleTrustReport {
    pub(crate) server_ed25519_public_key_configured: bool,
}

impl Repository {
    pub(crate) async fn upsert_agent_identity(
        &self,
        request: &UpsertAgentIdentityRequest,
        operator: &AuthContext,
    ) -> Result<AgentIdentityView> {
        let client_id = match request.client_id.as_deref() {
            Some(id) if !id.trim().is_empty() => id.trim().to_string(),
            _ => self.generate_auto_client_id().await?,
        };
        let public_key = decode_public_key_hex(&request.client_public_key_hex)?;
        let public_key_sha256_hex = public_key_sha256_hex(&public_key);
        let display_name = request
            .display_name
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| client_id.as_str())
            .to_string();
        let tags = normalize_tags(&request.tags);

        if self
            .is_client_key_revoked(&client_id, &public_key)
            .await
            .context("failed to check agent key revocation before identity import")?
        {
            anyhow::bail!("agent_identity_key_revoked");
        }

        match self {
            Self::Memory(memory) => {
                if memory.hidden_clients.read().await.contains(&client_id) {
                    anyhow::bail!("agent_identity_deactivated");
                }
                if memory
                    .agents
                    .read()
                    .await
                    .iter()
                    .any(|agent| {
                        agent.id == client_id && matches!(agent.status.as_str(), "revoked" | "deleted")
                    })
                {
                    anyhow::bail!("agent_identity_deactivated");
                }
                let existing = memory.client_public_keys.read().await.get(&client_id).cloned();
                if request.replace_existing_key {
                    if existing.as_ref().is_none_or(|key| key.is_empty()) {
                        anyhow::bail!("client_not_found_or_no_key");
                    }
                } else {
                    if existing.is_some()
                        || memory
                            .agents
                            .read()
                            .await
                            .iter()
                            .any(|agent| agent.id == client_id)
                    {
                        anyhow::bail!("client_id_already_registered");
                    }
                }
                memory
                    .client_public_keys
                    .write()
                    .await
                    .insert(client_id.to_string(), public_key.clone());
                {
                    let mut known_tags = memory.tags.write().await;
                    for tag in &tags {
                        if !known_tags.iter().any(|known| known == tag) {
                            known_tags.push(tag.clone());
                        }
                    }
                    known_tags.sort();
                }
                let view = {
                    let mut agents = memory.agents.write().await;
                    if let Some(agent) = agents.iter_mut().find(|agent| agent.id == client_id) {
                        if matches!(agent.status.as_str(), "revoked" | "deleted") {
                            anyhow::bail!("agent_identity_deactivated");
                        }
                        if request.display_name.is_some() {
                            agent.display_name = display_name.clone();
                        }
                        for tag in &tags {
                            if !agent.tags.iter().any(|existing| existing == tag) {
                                agent.tags.push(tag.clone());
                            }
                        }
                        agent.tags.sort();
                        AgentIdentityView {
                            client_id: agent.id.clone(),
                            display_name: agent.display_name.clone(),
                            status: agent.status.clone(),
                            current_public_key_sha256_hex: public_key_sha256_hex.clone(),
                            tags: agent.tags.clone(),
                        }
                    } else {
                        let mut agent_tags = tags.clone();
                        agent_tags.sort();
                        agents.push(AgentView {
                            id: client_id.to_string(),
                            display_name: display_name.clone(),
                            status: "never".to_string(),
                            tags: agent_tags.clone(),
                            registration_ip: None,
                            last_ip: None,
                            last_seen_at: None,
                            internal_build_number: 1,
                            stale_since: None,
                            stale_reason: None,
                            capabilities: vpsman_common::AgentCapabilitySnapshot::default(),
                        });
                        AgentIdentityView {
                            client_id: client_id.to_string(),
                            display_name,
                            status: "offline".to_string(),
                            current_public_key_sha256_hex: public_key_sha256_hex.clone(),
                            tags: agent_tags,
                        }
                    }
                };
                memory.audits.write().await.push(AuditLogView {
                    id: Uuid::new_v4(),
                    actor_id: Some(operator.operator.id),
                    action: "agent_identity.upserted".to_string(),
                    target: format!("client:{client_id}"),
                    command_hash: None,
                    metadata: json!({
                        "client_id": client_id,
                        "public_key_sha256_hex": public_key_sha256_hex,
                        "replace_existing_key": request.replace_existing_key,
                        "tags": tags,
                    }),
                    created_at: unix_now().to_string(),
                });
                Ok(view)
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                if fetch_postgres_key_revocation(&mut tx, &client_id, &public_key_sha256_hex)
                    .await?
                    .is_some()
                {
                    anyhow::bail!("agent_identity_key_revoked");
                }
                let existing = sqlx::query(
                    r#"
                    SELECT id, display_name, status, public_key, hidden_at IS NOT NULL AS hidden
                    FROM clients
                    WHERE id = $1
                    FOR UPDATE
                    "#,
                )
                .bind(&client_id)
                .fetch_optional(&mut *tx)
                .await?;

                if let Some(row) = existing.as_ref() {
                    let hidden: bool = row.try_get("hidden")?;
                    let status: String = row.try_get("status")?;
                    if hidden || matches!(status.as_str(), "revoked" | "deleted") {
                        anyhow::bail!("agent_identity_deactivated");
                    }
                    let existing_key: Vec<u8> = row.try_get("public_key")?;
                    if request.replace_existing_key {
                        if existing_key.is_empty() {
                            anyhow::bail!("client_not_found_or_no_key");
                        }
                    } else {
                        anyhow::bail!("client_id_already_registered");
                    }
                    sqlx::query(
                        r#"
                        UPDATE clients
                        SET display_name = CASE WHEN $2::text IS NULL THEN display_name ELSE $2 END,
                            public_key = $3,
                            stale_since = NULL,
                            stale_reason = NULL,
                            stale_build_number = NULL
                        WHERE id = $1 AND hidden_at IS NULL
                        "#,
                    )
                    .bind(&client_id)
                    .bind(request.display_name.as_deref().map(str::trim).filter(|value| !value.is_empty()))
                    .bind(&public_key)
                    .execute(&mut *tx)
                    .await?;
                } else {
                    sqlx::query(
                        r#"
                        INSERT INTO clients (
                            id, display_name, public_key, status, internal_build_number, capabilities
                        )
                        VALUES ($1, $2, $3, 'never', 1, '{}'::jsonb)
                        "#,
                    )
                    .bind(&client_id)
                    .bind(&display_name)
                    .bind(&public_key)
                    .execute(&mut *tx)
                    .await?;
                }

                for tag in &tags {
                    let tag_id = upsert_postgres_tag(&mut tx, tag).await?;
                    sqlx::query(
                        r#"
                        INSERT INTO client_tags (client_id, tag_id)
                        VALUES ($1, $2)
                        ON CONFLICT DO NOTHING
                        "#,
                    )
                    .bind(&client_id)
                    .bind(tag_id)
                    .execute(&mut *tx)
                    .await?;
                }

                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (id, actor_id, action, target, command_hash, metadata)
                    VALUES ($1, $2, 'agent_identity.upserted', $3, NULL, $4)
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind(operator.operator.id)
                .bind(format!("client:{}", &client_id))
                .bind(json!({
                    "client_id": &client_id,
                    "public_key_sha256_hex": public_key_sha256_hex,
                    "replace_existing_key": request.replace_existing_key,
                    "tags": tags,
                }))
                .execute(&mut *tx)
                .await?;
                let view = fetch_postgres_agent_identity(&mut tx, &client_id).await?;
                tx.commit().await?;
                Ok(view)
            }
        }
    }

    async fn generate_auto_client_id(&self) -> Result<String> {
        match self {
            Self::Memory(memory) => {
                let max_numeric = memory
                    .agents
                    .read()
                    .await
                    .iter()
                    .filter_map(|agent| agent.id.parse::<u64>().ok())
                    .max()
                    .unwrap_or(0);
                Ok((max_numeric + 1).to_string())
            }
            Self::Postgres(pool) => {
                let row = sqlx::query(
                    r#"
                    SELECT COALESCE(MAX(id::bigint), 0) AS max_id
                    FROM clients
                    WHERE id ~ '^\d+$'
                    "#,
                )
                .fetch_one(pool)
                .await?;
                let max_id: i64 = row.try_get("max_id")?;
                Ok((max_id + 1).to_string())
            }
        }
    }

    pub(crate) async fn revoke_current_client_key(
        &self,
        client_id: &str,
        request: &CreateClientKeyRevocationRequest,
        operator: &AuthContext,
    ) -> Result<ClientKeyRevocationView> {
        let reason = normalized_reason(request.reason.as_deref());
        match self {
            Self::Memory(memory) => {
                if memory.hidden_clients.read().await.contains(client_id) {
                    anyhow::bail!("client not found: {client_id}");
                }
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
                memory.audits.write().await.push(AuditLogView {
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
                    SELECT public_key, status
                    FROM clients
                    WHERE id = $1 AND hidden_at IS NULL
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
                let prior_status: String = row.try_get("status")?;
                let public_key_sha256_hex = public_key_sha256_hex(&current_public_key);
                if let Some(existing) =
                    fetch_postgres_key_revocation(&mut tx, &client_id, &public_key_sha256_hex)
                        .await?
                {
                    mark_postgres_agent_revoked(
                        &mut tx,
                        client_id,
                        operator.operator.id,
                        reason.as_deref(),
                        &prior_status,
                    )
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
                mark_postgres_agent_revoked(
                    &mut tx,
                    client_id,
                    operator.operator.id,
                    reason.as_deref(),
                    &prior_status,
                )
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
                    fetch_postgres_key_revocation(&mut tx, &client_id, &public_key_sha256_hex)
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
                let hidden = memory.hidden_clients.read().await.clone();
                let public_keys = memory.client_public_keys.read().await.clone();
                let revocations = memory.client_key_revocations.read().await.clone();
                let mut current_key_revoked_count = 0usize;
                let mut clients = agents
                    .into_iter()
                    .filter(|agent| !hidden.contains(&agent.id))
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
                        if latest.is_some() {
                            current_key_revoked_count += 1;
                        }
                        KeyLifecycleClientView {
                            client_id: agent.id,
                            display_name: agent.display_name,
                            status: agent.status,
                            current_key_revoked: latest.is_some(),
                            current_public_key_sha256_hex,
                            latest_revoked_at: latest.map(|record| record.created_at.clone()),
                            latest_revocation_reason: latest.and_then(|record| record.reason.clone()),
                        }
                    })
                    .collect::<Vec<_>>();
                clients.sort_by(|left, right| {
                    left.display_name
                        .cmp(&right.display_name)
                        .then_with(|| left.client_id.cmp(&right.client_id))
                });
                let direct_identity_client_count = clients
                    .iter()
                    .filter(|client| client.current_public_key_sha256_hex.is_some())
                    .count();
                Ok(KeyLifecycleReportView {
                    server_ed25519_public_key_configured: trust
                        .server_ed25519_public_key_configured,
                    direct_identity_client_count,
                    current_key_revoked_count,
                    revocation_count: revocations.len(),
                    clients,
                })
            }
            Self::Postgres(pool) => {
                let client_rows = sqlx::query(
                    r#"
                    SELECT id, display_name, status, public_key
                    FROM clients
                    WHERE hidden_at IS NULL
                    ORDER BY display_name, id
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
                            latest_revocation_reason: latest.and_then(|record| record.reason.clone()),
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;
                let direct_identity_client_count = clients
                    .iter()
                    .filter(|client| client.current_public_key_sha256_hex.is_some())
                    .count();
                Ok(KeyLifecycleReportView {
                    server_ed25519_public_key_configured: trust
                        .server_ed25519_public_key_configured,
                    direct_identity_client_count,
                    current_key_revoked_count,
                    revocation_count: revocations.len(),
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
            Self::Memory(memory) => Ok(memory
                .client_key_revocations
                .read()
                .await
                .iter()
                .any(|record| {
                    record.client_id == client_id
                        && record.public_key_sha256_hex == public_key_sha256_hex
                })),
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

    pub(crate) async fn client_public_key_sha256_hex(
        &self,
        client_id: &str,
    ) -> Result<Option<String>> {
        match self {
            Self::Memory(memory) => {
                if memory.hidden_clients.read().await.contains(client_id) {
                    return Ok(None);
                }
                Ok(memory
                    .client_public_keys
                    .read()
                    .await
                    .get(client_id)
                    .filter(|key| !key.is_empty())
                    .map(|key| public_key_sha256_hex(key)))
            }
            Self::Postgres(pool) => {
                let row = sqlx::query(
                    r#"
                    SELECT public_key
                    FROM clients
                    WHERE id = $1 AND hidden_at IS NULL
                    "#,
                )
                .bind(client_id)
                .fetch_optional(pool)
                .await?;
                let Some(row) = row else {
                    return Ok(None);
                };
                let public_key: Vec<u8> = row.try_get("public_key")?;
                Ok((!public_key.is_empty()).then(|| public_key_sha256_hex(&public_key)))
            }
        }
    }
}

async fn mark_memory_agent_revoked(memory: &crate::repository::MemoryState, client_id: &str) {
    let now = unix_now().to_string();
    memory
        .hidden_clients
        .write()
        .await
        .insert(client_id.to_string());
    if let Some(agent) = memory
        .agents
        .write()
        .await
        .iter_mut()
        .find(|agent| agent.id == client_id)
    {
        agent.status = "revoked".to_string();
        agent.stale_since = None;
        agent.stale_reason = None;
    }
    for session in memory.gateway_sessions.write().await.iter_mut() {
        if session.client_id == client_id && session.status == "active" {
            session.status = "ended".to_string();
            session.last_seen_at = now.clone();
            session.ended_at = Some(now.clone());
            session.end_reason = Some("client_key_revoked".to_string());
        }
    }
}

async fn mark_postgres_agent_revoked(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    client_id: &str,
    operator_id: Uuid,
    request_reason: Option<&str>,
    prior_status: &str,
) -> Result<()> {
    let hidden_reason = request_reason.unwrap_or("client_key_revoked");
    sqlx::query(
        r#"
        UPDATE clients
        SET
            hidden_at = COALESCE(hidden_at, now()),
            hidden_by = COALESCE(hidden_by, $2),
            hidden_reason = COALESCE($3, hidden_reason),
            status = 'revoked',
            stale_since = NULL,
            stale_reason = NULL,
            stale_build_number = NULL
        WHERE id = $1 AND hidden_at IS NULL
        "#,
    )
    .bind(client_id)
    .bind(operator_id)
    .bind(hidden_reason)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        r#"
        UPDATE gateway_sessions
        SET
            status = 'ended',
            last_seen_at = now(),
            ended_at = COALESCE(ended_at, now()),
            end_reason = COALESCE(end_reason, 'client_key_revoked')
        WHERE client_id = $1 AND status = 'active'
        "#,
    )
    .bind(client_id)
    .execute(&mut **tx)
    .await?;
    if prior_status != "revoked" {
        sqlx::query(
            r#"
            INSERT INTO client_status_history (
                id, client_id, from_status, to_status, reason, metadata
            )
            VALUES ($1, $2, $3, 'revoked', 'client_key_revoked', $4)
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(client_id)
        .bind(prior_status)
        .bind(json!({
            "reason": request_reason,
            "frontend_visible": false,
            "access_deactivated": true,
        }))
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

async fn fetch_postgres_agent_identity(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    client_id: &str,
) -> Result<AgentIdentityView> {
    let row = sqlx::query(
        r#"
        SELECT
            c.id,
            c.display_name,
            c.status,
            c.public_key,
            COALESCE(array_remove(array_agg(t.name ORDER BY t.name), NULL), ARRAY[]::TEXT[]) AS tags
        FROM clients c
        LEFT JOIN client_tags ct ON ct.client_id = c.id
        LEFT JOIN tags t ON t.id = ct.tag_id
        WHERE c.id = $1 AND c.hidden_at IS NULL
        GROUP BY c.id, c.display_name, c.status, c.public_key
        "#,
    )
    .bind(client_id)
    .fetch_one(&mut **tx)
    .await?;
    let public_key: Vec<u8> = row.try_get("public_key")?;
    Ok(AgentIdentityView {
        client_id: row.try_get("id")?,
        display_name: row.try_get("display_name")?,
        status: row.try_get("status")?,
        current_public_key_sha256_hex: public_key_sha256_hex(&public_key),
        tags: row.try_get("tags")?,
    })
}

async fn upsert_postgres_tag(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tag: &str,
) -> Result<Uuid> {
    let row = sqlx::query(
        r#"
        INSERT INTO tags (id, name)
        VALUES ($1, $2)
        ON CONFLICT (name) DO UPDATE SET name = EXCLUDED.name
        RETURNING id
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(tag)
    .fetch_one(&mut **tx)
    .await?;
    Ok(row.try_get("id")?)
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

fn normalized_reason(reason: Option<&str>) -> Option<String> {
    reason
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.chars().take(1024).collect())
}

fn decode_public_key_hex(value: &str) -> Result<Vec<u8>> {
    let public_key = hex::decode(value.trim()).context("invalid agent public key hex")?;
    anyhow::ensure!(public_key.len() == 32, "agent public key must be 32 bytes");
    Ok(public_key)
}

fn normalize_tags(tags: &[String]) -> Vec<String> {
    let mut normalized = tags
        .iter()
        .map(|tag| tag.trim().to_string())
        .filter(|tag| !tag.is_empty())
        .collect::<Vec<_>>();
    normalized.sort();
    normalized.dedup();
    normalized
}

pub(crate) fn public_key_sha256_hex(public_key: &[u8]) -> String {
    hex::encode(Sha256::digest(public_key))
}
